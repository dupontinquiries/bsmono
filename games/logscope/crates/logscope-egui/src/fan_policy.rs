//! How wide to fan, decided OUTSIDE the queen.
//!
//! # Why the queen does not get to choose
//!
//! Asked how many workers to spawn, a model will answer with a number that
//! sounds right rather than one the machine can serve — three because there
//! are three files, seven because the list had seven entries. Neither is
//! informed by whether the engine has a slot free or whether the budget can
//! pay for seven sets of findings.
//!
//! So the queen PROPOSES and this DISPOSES. [`decide`] takes what the queen
//! wants and clamps it against three facts it cannot see:
//!
//! * how many distinct possibilities the survey actually found,
//! * how much of the token budget is left,
//! * how many generations the engine can have in flight.
//!
//! The queen keeps partial control — its proposal is respected when it is
//! affordable — and cannot produce a strange number of threads.
//!
//! # Fanning is not free and not always faster
//!
//! Each worker costs a minimum slice of the budget whether or not it finds
//! anything, and on a single-slot engine four workers are four turns in a
//! queue, which is slower than one worker reading three files because each
//! pays its own prompt again.
//!
//! So the rule is: fan when there is genuine ambiguity AND the room to explore
//! it AND somewhere to run it. Any one missing and a single worker is both
//! cheaper and faster.

/// What the engine can actually take right now.
#[derive(Debug, Clone, Copy)]
pub struct Capacity {
    /// Generations the engine can have in flight at once. 1 means serial.
    pub slots: u16,
    /// Already in flight or queued. Fanning into a full queue adds latency
    /// without adding parallelism, which is the case that looks like
    /// concurrency and behaves like a backlog.
    pub busy: u16,
}

impl Capacity {
    pub fn free(&self) -> u16 {
        self.slots.saturating_sub(self.busy)
    }
}

/// Why the fan came out the width it did. Reported so the operator can see
/// which constraint bound it — "why only one worker" has four possible
/// answers and guessing between them wastes an afternoon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bound {
    /// The queen's own proposal was the smallest number.
    AsProposed,
    /// Only one real possibility, so nothing to fan across.
    Unambiguous,
    /// The budget could not pay for more workers.
    Budget,
    /// The engine had no more free slots.
    Slots,
    /// [`MAX_FAN`] — the backstop.
    Ceiling,
}

/// The most workers that may ever run on one question.
///
/// Not a performance number: a correctness one. Beyond a handful the findings
/// stop being composable — the queen has to reconcile six partial answers
/// inside the same window it was trying to conserve, which is the problem this
/// whole subsystem exists to avoid.
pub const MAX_FAN: u16 = 4;

/// Tokens a worker needs to be worth spawning at all.
///
/// A worker that can only afford one short finding will return something
/// technically true and useless. Below this, spend the budget on one worker
/// that can actually look.
pub const MIN_TOKENS_PER_WORKER: u32 = 800;

/// Decide the fan width. Returns `(workers, why)`; `workers` is at least 1.
pub fn decide(proposed: u16, possibilities: u16, budget_remaining: u32, cap: Capacity)
    -> (u16, Bound)
{
    // Ceiling first, so nothing below can hand back more than MAX_FAN.
    let mut n = proposed.min(MAX_FAN);
    let mut why = if proposed > MAX_FAN { Bound::Ceiling } else { Bound::AsProposed };

    // One possibility is not a fan, whatever anyone proposed.
    if possibilities <= 1 {
        return (1, Bound::Unambiguous);
    }
    if possibilities < n {
        n = possibilities;
        why = Bound::Unambiguous;
    }

    let affordable = (budget_remaining / MIN_TOKENS_PER_WORKER) as u16;
    if affordable < n {
        n = affordable;
        why = Bound::Budget;
    }

    // `free()` and not `slots`: fanning into a full queue adds latency without
    // parallelism. At least one always runs — a busy engine means waiting, not
    // abandoning the question.
    let free = cap.free().max(1);
    if free < n {
        n = free;
        why = Bound::Slots;
    }

    (n.max(1), if n <= 1 && why == Bound::AsProposed { Bound::AsProposed } else { why })
}

/// Would fanning to `n` be faster than one worker doing the same reading?
///
/// Only when the engine can genuinely run them at once. On a serial engine
/// `n` workers are `n` queued turns, each re-paying its own prompt, so the
/// fan is strictly slower AND more expensive than one worker with a list.
pub fn fan_is_faster(n: u16, cap: Capacity) -> bool {
    n > 1 && cap.free() >= n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(slots: u16, busy: u16) -> Capacity {
        Capacity { slots, busy }
    }

    #[test]
    fn the_queen_cannot_spawn_a_strange_number_of_threads() {
        // The reason the decision is not the queen's. Asked for a number, a
        // model gives one that sounds right — and 40 sounds fine next to a
        // list of 40 files.
        let (n, why) = decide(40, 40, 1_000_000, cap(64, 0));
        assert_eq!(n, MAX_FAN);
        assert_eq!(why, Bound::Ceiling);
    }

    #[test]
    fn one_possibility_is_never_fanned() {
        // Nothing to explore. Two workers on one file is two prompts for one
        // answer, whatever the budget allows.
        let (n, why) = decide(4, 1, 1_000_000, cap(64, 0));
        assert_eq!((n, why), (1, Bound::Unambiguous));
        // And zero possibilities still runs one worker rather than none —
        // a survey that found nothing is a question, not a dead end.
        assert_eq!(decide(4, 0, 1_000_000, cap(8, 0)).0, 1);

        // Proposed 1 AND one possibility must still report Unambiguous, not
        // AsProposed. Found by deleting the early return and watching nothing
        // fail: every case I had written proposed more than one, so the
        // general clamp below produced the same answer and the reason was the
        // only thing that differed. The reason is the point — it is what tells
        // the operator there was nothing to fan across rather than that the
        // queen simply asked for one.
        assert_eq!(decide(1, 1, 1_000_000, cap(8, 0)), (1, Bound::Unambiguous));
    }

    #[test]
    fn a_thin_budget_narrows_the_fan_rather_than_starving_every_worker() {
        // Four workers with 900 tokens between them each get one short
        // finding and return something true and useless. One worker with 900
        // can actually look.
        let (n, why) = decide(4, 4, 900, cap(64, 0));
        assert_eq!((n, why), (1, Bound::Budget));
        // Enough for three, and three is what runs.
        let (n, why) = decide(4, 4, 3 * MIN_TOKENS_PER_WORKER, cap(64, 0));
        assert_eq!((n, why), (3, Bound::Budget));
    }

    #[test]
    fn a_busy_engine_bounds_the_fan_by_free_slots_not_total() {
        // Fanning into a full queue looks like concurrency and behaves like a
        // backlog: the same work, later.
        let (n, why) = decide(4, 4, 1_000_000, cap(4, 3));
        assert_eq!((n, why), (1, Bound::Slots));
        // Fully busy still runs one — waiting, not abandoning the question.
        assert_eq!(decide(4, 4, 1_000_000, cap(4, 4)).0, 1);
    }

    #[test]
    fn the_queens_proposal_is_respected_when_it_fits() {
        // Partial control means partial: a proposal the machine can serve is
        // not second-guessed.
        let (n, why) = decide(2, 4, 1_000_000, cap(8, 0));
        assert_eq!((n, why), (2, Bound::AsProposed));
        let (n, _) = decide(3, 3, 1_000_000, cap(8, 0));
        assert_eq!(n, 3);
    }

    #[test]
    fn the_tightest_constraint_is_the_one_reported() {
        // "Why only one worker" has four answers and guessing between them
        // wastes an afternoon. Budget is tighter than slots here, and budget
        // is what must be named.
        let (n, why) = decide(4, 4, MIN_TOKENS_PER_WORKER, cap(4, 0));
        assert_eq!((n, why), (1, Bound::Budget));
        // Slots tighter than budget: slots is named.
        let (n, why) = decide(4, 4, 1_000_000, cap(2, 0));
        assert_eq!((n, why), (2, Bound::Slots));
    }

    #[test]
    fn a_serial_engine_is_never_told_a_fan_would_be_faster() {
        // n workers on one slot are n queued turns, each re-paying its prompt
        // — strictly slower AND dearer than one worker with a list.
        assert!(!fan_is_faster(4, cap(1, 0)));
        assert!(!fan_is_faster(4, cap(8, 6)));
        assert!(!fan_is_faster(1, cap(8, 0)), "a fan of one is not a fan");
        assert!(fan_is_faster(4, cap(8, 0)));
    }

    #[test]
    fn the_result_is_always_runnable() {
        // Never zero, never past the ceiling, whatever it is handed —
        // including the degenerate inputs a caller will eventually pass.
        for proposed in [0u16, 1, 3, 9, u16::MAX] {
            for poss in [0u16, 1, 5, u16::MAX] {
                for budget in [0u32, 799, 5_000] {
                    for c in [cap(0, 0), cap(1, 9), cap(8, 0)] {
                        let (n, _) = decide(proposed, poss, budget, c);
                        assert!((1..=MAX_FAN).contains(&n),
                                "{n} out of range for {proposed}/{poss}/{budget}/{c:?}");
                    }
                }
            }
        }
    }
}
