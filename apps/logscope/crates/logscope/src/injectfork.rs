//! injectfork.rs — an interruption forks the graph instead of discarding a turn.
//!
//! ## What was wrong with choosing
//!
//! `injecttime` decides interrupt-or-queue and throws the loser away. Near the crossover that decision
//! is close to a coin flip, and choosing wrong costs a whole turn: you discover the queue was wrong,
//! and re-run. **Running both costs one extra turn.** So where the arithmetic is ambiguous, forking is
//! cheaper in expectation than deciding — and ambiguity is exactly where a fork is most informative,
//! because the two branches will actually differ.
//!
//! Away from the crossover one option dominates and forking wastes capacity for a comparison nobody
//! needs. So the fork window is narrow on purpose.
//!
//! ## And the discarded branch was never worthless
//!
//! An interrupted turn is what the debater was going to say before you spoke. Compared against what it
//! said after, that is the only direct evidence of whether the interjection helped — the same shape as
//! `panelvoice`'s blind control, arrived at from the other side. Discarding it destroys the comparison
//! that motivated interrupting.
//!
//! So both branches become nodes in the conversation graph. `branchtree` already treats branches as
//! first class and exports every one, so a fork is a side-path rather than a special case.
//!
//! ## Why queueing both is cheap even with no room to run them
//!
//! The engine keeps a KV prefix tree. Both branches share the prompt prefix, so the second one
//! re-prefills only the diverging suffix — the expensive part is already cached. That is what makes
//! "queue both, keep both warm" affordable when "run both now" is not.

/// What the scheduler can afford right now.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Capacity {
    /// A second branch can run concurrently.
    Room,
    /// Nothing spare; branches must be serialised.
    Full,
}

/// What to do with an injection, once forking is on the table.
#[derive(Clone, PartialEq, Debug)]
pub enum Plan {
    /// Run both now and compare when they finish.
    ForkNow { keep_running: bool },
    /// Both become graph nodes; one runs, the other holds a warm cache for later.
    ForkQueued { runs_first: Branch, warm: Branch },
    /// The decision is not close; take the clear option and do not spend capacity on a comparison.
    Single { interrupt: bool, why: String },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Branch {
    /// The turn as it was going, with the injection held for afterwards.
    Uninterrupted,
    /// Restarted with the injection in context.
    Redirected,
}

/// How close to the crossover counts as ambiguous.
///
/// Narrow on purpose: outside this band one option dominates and a fork buys a comparison nobody needs.
pub const AMBIGUOUS: f32 = 0.35;

/// Decide, given the same inputs `injecttime` uses plus what the scheduler has spare.
///
/// `ratio` is worth-of-restarting over work-already-done — 1.0 is the crossover.
pub fn plan(ratio: f32, cap: Capacity) -> Plan {
    let ambiguous = (ratio - 1.0).abs() <= AMBIGUOUS;
    if !ambiguous {
        let interrupt = ratio > 1.0;
        return Plan::Single { interrupt, why: if interrupt {
            format!("restarting is worth {ratio:.1}x what it discards — not close enough to be worth \
running both")
        } else {
            format!("restarting is worth only {ratio:.1}x what it discards; the turn is far enough \
along that the comparison would cost more than it tells you")
        }};
    }
    match cap {
        Capacity::Room => Plan::ForkNow { keep_running: true },
        // Serialised, but BOTH are kept: the loser holds a warm cache, and both share the prompt
        // prefix so resuming it re-prefills only the diverging suffix.
        Capacity::Full => Plan::ForkQueued {
            runs_first: if ratio >= 1.0 { Branch::Redirected } else { Branch::Uninterrupted },
            warm: if ratio >= 1.0 { Branch::Uninterrupted } else { Branch::Redirected },
        },
    }
}

/// Never discard the branch that was interrupted.
///
/// It is what the debater was going to say before you spoke, and compared against what it said after,
/// it is the only direct evidence of whether the interjection helped.
pub const PRESERVE_INTERRUPTED: bool = true;

/// What a completed fork tells you.
#[derive(Clone, PartialEq, Debug)]
pub enum ForkReading {
    /// The injection changed the answer. It was worth making.
    Redirected { before: String, after: String },
    /// Both branches landed in the same place — the debater was already going there.
    Redundant,
}

/// Compare the two branches.
pub fn read_fork(uninterrupted: &str, redirected: &str) -> ForkReading {
    let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    if norm(uninterrupted) == norm(redirected) { ForkReading::Redundant }
    else { ForkReading::Redirected { before: uninterrupted.into(), after: redirected.into() } }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_close_call_with_room_runs_BOTH() {
        // Choosing wrong near the crossover costs a whole turn; running both costs one extra. So where
        // the arithmetic is ambiguous, forking is cheaper in expectation than deciding.
        assert_eq!(plan(1.0, Capacity::Room), Plan::ForkNow { keep_running: true });
        assert_eq!(plan(1.2, Capacity::Room), Plan::ForkNow { keep_running: true });
    }

    #[test]
    fn a_clear_call_does_NOT_spend_capacity_on_a_comparison() {
        // Outside the band one option dominates and the two branches would agree — a fork buys a
        // comparison nobody needs. The window is narrow on purpose.
        match plan(4.0, Capacity::Room) {
            Plan::Single { interrupt, why } => {
                assert!(interrupt);
                assert!(why.contains("not close enough"));
            }
            other => panic!("expected Single, got {other:?}"),
        }
        assert!(matches!(plan(0.2, Capacity::Room), Plan::Single { interrupt: false, .. }));
    }

    #[test]
    fn no_room_still_keeps_BOTH_branches() {
        // The refinement: serialise them, do not discard one. The loser holds a warm cache, and both
        // share the prompt prefix — so resuming re-prefills only the diverging suffix, which is what
        // makes "queue both" affordable when "run both" is not.
        match plan(1.1, Capacity::Full) {
            Plan::ForkQueued { runs_first, warm } => {
                assert_eq!(runs_first, Branch::Redirected, "the favoured one runs");
                assert_eq!(warm, Branch::Uninterrupted, "and the other is kept, not dropped");
            }
            other => panic!("expected ForkQueued, got {other:?}"),
        }
    }

    #[test]
    fn the_favoured_branch_flips_with_the_ratio() {
        // Below the crossover the uninterrupted turn is the better bet, so it runs and the redirect
        // waits — the queue order is not fixed.
        match plan(0.8, Capacity::Full) {
            Plan::ForkQueued { runs_first, warm } => {
                assert_eq!(runs_first, Branch::Uninterrupted);
                assert_eq!(warm, Branch::Redirected);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_fork_that_lands_in_the_SAME_place_says_the_injection_was_redundant() {
        // Worth knowing and only visible because the interrupted branch was preserved: the debater was
        // already going there, so the interjection cost a turn and changed nothing.
        assert_eq!(read_fork("use a BTreeMap here", "Use a btreemap  here"), ForkReading::Redundant);
    }

    #[test]
    fn a_fork_that_diverges_shows_BOTH_sides() {
        // The comparison that motivated interrupting. Discarding the first branch destroys it, which
        // is what injecttime did.
        match read_fork("use a HashMap", "use a BTreeMap") {
            ForkReading::Redirected { before, after } => {
                assert!(before.contains("HashMap") && after.contains("BTreeMap"));
            }
            other => panic!("{other:?}"),
        }
        assert!(PRESERVE_INTERRUPTED);
    }
}
