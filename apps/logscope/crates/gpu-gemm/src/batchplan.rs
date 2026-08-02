//! batchplan.rs — how wide to run a batch, for every source of independent rows.
//!
//! Three places produce rows that could share one weight pass instead of taking one each:
//!
//! * **the liaison reading a debate** — N debater outputs against one prompt, N independent forward
//!   passes over identical weights;
//! * **a fan-out** — several branches sharing a prefilled ancestor and differing only in their tails;
//! * **file reading** — N files prefilled over the same weights.
//!
//! None of them is speculation. Nothing is guessed and nothing is rejected, so unlike a draft-verify
//! loop there is no accept rate to worry about and no wasted work when the batch is wide. It is
//! batching that was always available and never taken.
//!
//! ## Why one module decides
//!
//! The constraints are shared and non-obvious, and three call sites would get them subtly different —
//! which is the duplicated-policy failure this codebase has already paid for twice. The ceiling is not
//! "as wide as possible":
//!
//! * **above m=8 the q4 path falls to the strided kernel** that re-reads weights per tile, so a wider
//!   batch moves work onto a kernel that undoes the saving. Nine rows are slower than eight.
//! * **width lengthens a dispatch**, and a long dispatch is what the GPU watchdog kills. That is not
//!   hypothetical here: a Qwen3-30B run hung after `windowed sampling ON` with GPU timeouts flooding
//!   the kernel log.
//! * **a batch of one is not a batch** — it must not pay any setup a single pass would not.

/// The kernel's own limit. Above this, `LINEAR_Q4` re-reads weights per tile.
///
/// Not a tuning knob: it is where `MATVEC_SMALL_Q4` stops and a different kernel begins.
pub const KERNEL_MAX_ROWS: usize = 8;

/// Why a batch is narrower than the number of rows available.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Narrowed {
    /// Everything available fits.
    NotNarrowed,
    /// Capped by the batched kernel's row limit.
    KernelLimit,
    /// Capped because the device kills long dispatches.
    WatchdogBudget,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BatchPlan {
    /// Rows per pass. 1 means run singly — the caller should take its ordinary path.
    pub width: usize,
    /// How many passes the available rows take at this width.
    pub passes: usize,
    pub why: Narrowed,
}

impl BatchPlan {
    /// Is batching worth taking at all?
    ///
    /// A width of 1 means the plan found no saving, and the caller should use the path it already has
    /// rather than a batch loop that happens to have one element — same work, more machinery.
    pub fn worth_it(&self) -> bool { self.width > 1 }

    /// Weight passes saved against running every row singly.
    pub fn passes_saved(&self, rows: usize) -> usize { rows.saturating_sub(self.passes) }
}

/// Plan a batch of `rows` independent streams.
///
/// `watchdog_rows` is the widest dispatch this device is believed to survive — `None` on a device with
/// no watchdog, where only the kernel limit applies.
pub fn plan(rows: usize, watchdog_rows: Option<usize>) -> BatchPlan {
    if rows <= 1 {
        return BatchPlan { width: 1, passes: rows, why: Narrowed::NotNarrowed };
    }
    let mut width = rows.min(KERNEL_MAX_ROWS);
    let mut why = if rows > KERNEL_MAX_ROWS { Narrowed::KernelLimit } else { Narrowed::NotNarrowed };
    if let Some(w) = watchdog_rows {
        // The watchdog wins when it is tighter, because exceeding it is a hang rather than a slowdown.
        // A slow batch is a cost; a killed dispatch is a dead session.
        if w < width {
            width = w.max(1);
            why = Narrowed::WatchdogBudget;
        }
    }
    BatchPlan { width, passes: rows.div_ceil(width.max(1)), why }
}

/// How to spend a fixed width between certain work and speculative work.
///
/// Branches are work that MUST happen; drafts are guesses that may be discarded. Given both, the
/// width goes to branches first — a rejected draft wastes a row that a branch would have used
/// productively, and there is no symmetric loss the other way.
///
/// Returns `(branch_rows, draft_rows)`.
pub fn split_width(width: usize, branches: usize, want_drafts: usize) -> (usize, usize) {
    let b = branches.clamp(1, width.max(1));
    (b, want_drafts.min(width.saturating_sub(b)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_five_way_panel_batches_in_ONE_pass() {
        // The liaison case: five debater outputs against one prompt, no speculation, nothing rejected.
        let p = plan(5, None);
        assert_eq!(p.width, 5);
        assert_eq!(p.passes, 1);
        assert_eq!(p.why, Narrowed::NotNarrowed);
        assert_eq!(p.passes_saved(5), 4, "four weight passes that would otherwise be paid");
    }

    #[test]
    fn above_the_kernel_limit_it_takes_SEVERAL_passes_rather_than_one_wide_one() {
        // Above m=8 the q4 path falls to the strided kernel that re-reads weights per tile — nine rows
        // in one pass are slower than nine rows in two.
        let p = plan(20, None);
        assert_eq!(p.width, KERNEL_MAX_ROWS);
        assert_eq!(p.passes, 3);
        assert_eq!(p.why, Narrowed::KernelLimit);
    }

    #[test]
    fn the_WATCHDOG_wins_when_it_is_tighter_than_the_kernel() {
        // Exceeding the kernel limit is a slowdown; exceeding the watchdog is a hang. A Qwen3-30B run
        // stopped dead after "windowed sampling ON" with GPU timeouts flooding the kernel log, so
        // this is not a hypothetical ordering.
        let p = plan(8, Some(3));
        assert_eq!(p.width, 3);
        assert_eq!(p.why, Narrowed::WatchdogBudget);
        assert_eq!(p.passes, 3);
    }

    #[test]
    fn a_looser_watchdog_does_not_RAISE_the_kernel_limit() {
        // A device that survives wide dispatches still cannot use the batched kernel above 8.
        let p = plan(20, Some(64));
        assert_eq!(p.width, KERNEL_MAX_ROWS);
        assert_eq!(p.why, Narrowed::KernelLimit);
    }

    #[test]
    fn a_single_row_is_NOT_a_batch() {
        // The caller must take its ordinary path rather than a batch loop with one element: same work,
        // more machinery, and a new code path to be wrong in.
        let p = plan(1, None);
        assert!(!p.worth_it());
        assert_eq!(p.width, 1);
        assert_eq!(p.passes_saved(1), 0);
    }

    #[test]
    fn zero_rows_does_not_divide_by_zero() {
        let p = plan(0, Some(4));
        assert!(!p.worth_it());
        assert_eq!(p.passes, 0);
    }

    #[test]
    fn width_goes_to_BRANCHES_before_drafts() {
        // Branches are work that must happen; drafts are guesses that may be discarded. A rejected
        // draft wastes a row a branch would have used, and there is no symmetric loss the other way.
        assert_eq!(split_width(8, 4, 4), (4, 4), "room for both");
        assert_eq!(split_width(4, 4, 4), (4, 0), "no room: branches keep it all");
        assert_eq!(split_width(4, 6, 2), (4, 0), "more branches than width: drafts get nothing");
        assert_eq!(split_width(8, 1, 8), (1, 7), "not fanning: drafts take the rest");
    }
}
