//! budget.rs — the control layer that makes "walk away and let the swarm work" safe.
//!
//! Two pieces, both pure and testable:
//!   * [`Budget`] / [`Spend`] — hard resource ceilings (tokens, wall-clock, attempts) per autonomous run.
//!     Autonomy without a budget is a runaway; nothing in the devloop should generate a token without a
//!     ceiling that can stop it.
//!   * [`Tier`] + [`survives_to`] — the graduated fitness cascade. Candidates are culled cheapest-gate
//!     first: typecheck (a millisecond, ruthless) → tests (seconds) → judge (a whole model call). A branch
//!     only reaches the expensive judge if it already cleared the cheap gates, so the swarm spends its
//!     tokens where they matter. This is the economic heart of the whole design — concentrate the
//!     strictness at the cheapest layer.

use std::time::Duration;

/// Hard ceilings for one autonomous run. Any one hit stops the loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    pub max_tokens: u64,   // total generation tokens across all branches/attempts
    pub max_wall_ms: u64,  // wall-clock ceiling
    pub max_attempts: u32, // total generate→verify cycles
}
impl Budget {
    pub fn new(max_tokens: u64, wall: Duration, max_attempts: u32) -> Self {
        Self { max_tokens, max_wall_ms: wall.as_millis() as u64, max_attempts }
    }
    /// A sane default for an unattended run: 500k tokens, 10 minutes, 40 cycles.
    pub fn default_run() -> Self { Self { max_tokens: 500_000, max_wall_ms: 600_000, max_attempts: 40 } }

    /// Which ceiling (if any) `spend` has crossed — `None` means keep going.
    pub fn exhausted(&self, spend: &Spend) -> Option<Reason> {
        if spend.tokens >= self.max_tokens { Some(Reason::Tokens) }
        else if spend.elapsed_ms >= self.max_wall_ms { Some(Reason::Time) }
        else if spend.attempts >= self.max_attempts { Some(Reason::Attempts) }
        else { None }
    }
    pub fn remaining_tokens(&self, spend: &Spend) -> u64 { self.max_tokens.saturating_sub(spend.tokens) }

    /// Fraction of the tightest budget consumed (0..=1) — for a progress bar / when to warn.
    pub fn fraction_used(&self, spend: &Spend) -> f32 {
        let f = |num: u64, den: u64| if den == 0 { 1.0 } else { (num as f32 / den as f32).min(1.0) };
        f(spend.tokens, self.max_tokens)
            .max(f(spend.elapsed_ms, self.max_wall_ms))
            .max(f(spend.attempts as u64, self.max_attempts as u64))
    }
}

/// Running totals for a run. The loop bumps these; the budget reads them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Spend {
    pub tokens: u64,
    pub elapsed_ms: u64,
    pub attempts: u32,
}
impl Spend {
    pub fn add_attempt(&mut self, tokens: u64, ms: u64) {
        self.tokens += tokens;
        self.elapsed_ms += ms;
        self.attempts += 1;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason { Tokens, Time, Attempts }
impl Reason {
    pub fn label(self) -> &'static str {
        match self { Reason::Tokens => "token budget", Reason::Time => "time budget", Reason::Attempts => "attempt budget" }
    }
}

// ── the graduated cascade ────────────────────────────────────────────────────────────────────────────────

/// A gate in the fitness cascade, cheapest first. A candidate must pass each to reach the next.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Compile = 0, // typecheck / validate — a millisecond, deterministic, ruthless (the fitness floor)
    Test = 1,    // run the tests — seconds
    Judge = 2,   // an LLM-judge rubric — a whole model call, the expensive last word
}
impl Tier {
    pub fn label(self) -> &'static str {
        match self { Tier::Compile => "compile", Tier::Test => "test", Tier::Judge => "judge" }
    }
    /// The ordered cascade.
    pub fn ladder() -> [Tier; 3] { [Tier::Compile, Tier::Test, Tier::Judge] }
    pub fn next(self) -> Option<Tier> {
        match self { Tier::Compile => Some(Tier::Test), Tier::Test => Some(Tier::Judge), Tier::Judge => None }
    }
}

/// Given the highest tier a candidate has PASSED (`None` = nothing run yet), the next gate to run — or
/// `None` when it's cleared the whole cascade. The rule that keeps the judge from ever seeing code that
/// doesn't compile.
pub fn next_gate(passed: Option<Tier>) -> Option<Tier> {
    match passed { None => Some(Tier::Compile), Some(t) => t.next() }
}

/// Whether a candidate that passed up to `passed` is allowed to spend the cost of tier `want` — i.e. it has
/// cleared every cheaper gate. Prevents burning a judge call on an uncompiled candidate.
pub fn survives_to(passed: Option<Tier>, want: Tier) -> bool {
    match want {
        Tier::Compile => true, // anything may attempt the cheapest gate
        _ => passed.map_or(false, |p| p >= want.prev()),
    }
}
impl Tier {
    fn prev(self) -> Tier {
        match self { Tier::Compile => Tier::Compile, Tier::Test => Tier::Compile, Tier::Judge => Tier::Test }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_stops_on_the_first_ceiling_hit() {
        let b = Budget { max_tokens: 1000, max_wall_ms: 10_000, max_attempts: 5 };
        let mut s = Spend::default();
        assert_eq!(b.exhausted(&s), None);
        s.add_attempt(999, 100);
        assert_eq!(b.exhausted(&s), None, "under all ceilings");
        s.add_attempt(2, 100); // tokens now 1001 ≥ 1000
        assert_eq!(b.exhausted(&s), Some(Reason::Tokens));
        // time ceiling
        let s2 = Spend { tokens: 0, elapsed_ms: 10_000, attempts: 0 };
        assert_eq!(b.exhausted(&s2), Some(Reason::Time));
        // attempts ceiling
        let s3 = Spend { tokens: 0, elapsed_ms: 0, attempts: 5 };
        assert_eq!(b.exhausted(&s3), Some(Reason::Attempts));
    }

    #[test]
    fn fraction_used_tracks_the_tightest_dimension() {
        let b = Budget { max_tokens: 1000, max_wall_ms: 1000, max_attempts: 100 };
        let s = Spend { tokens: 250, elapsed_ms: 800, attempts: 1 };
        assert!((b.fraction_used(&s) - 0.8).abs() < 1e-6, "time is the tightest at 80%");
        assert_eq!(b.remaining_tokens(&s), 750);
    }

    #[test]
    fn cascade_runs_cheap_to_expensive_and_gates_the_judge() {
        // fresh candidate → start at the cheapest gate
        assert_eq!(next_gate(None), Some(Tier::Compile));
        assert_eq!(next_gate(Some(Tier::Compile)), Some(Tier::Test));
        assert_eq!(next_gate(Some(Tier::Test)), Some(Tier::Judge));
        assert_eq!(next_gate(Some(Tier::Judge)), None, "cleared the whole cascade");
        // the judge never sees code that hasn't compiled + passed tests
        assert!(!survives_to(None, Tier::Judge), "uncompiled can't reach the judge");
        assert!(!survives_to(Some(Tier::Compile), Tier::Judge), "compiled-but-untested can't reach the judge");
        assert!(survives_to(Some(Tier::Test), Tier::Judge), "compiled+tested reaches the judge");
        // anything may attempt the cheapest gate
        assert!(survives_to(None, Tier::Compile));
        assert!(Tier::Compile < Tier::Judge, "tiers order cheap→expensive");
    }
}
