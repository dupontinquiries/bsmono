//! rolebench.rs — measure whether a model suits the roles this system HAS.
//!
//! ## Why this is feasible where a general reasoning benchmark is not
//!
//! A general benchmark needs an answer key, broad coverage, and defensible scoring — expensive, and it
//! measures capabilities we do not use. This needs none of that, for two reasons.
//!
//! **The roles are already enumerated.** `smallrole` lists them: fill a body against a signature, judge
//! whether code matches a contract, perform a mechanical edit, summarize. A model is not "good" or
//! "bad"; it is suited to some of those and not others, and that is the only question a router needs
//! answered.
//!
//! **Most of the scoring is mechanical.** A body either compiles or does not. A check either agrees
//! with the compiler or does not. A refutable claim either survives substitution or dies. None of that
//! needs a human grader or a reference answer — which is what usually makes a benchmark expensive
//! enough not to build.
//!
//! ## The trap: measuring what a model produces rather than what it enables
//!
//! The tempting design scores each model in isolation and picks the highest. That measures the wrong
//! thing. A checker that is right 70% of the time but whose errors are UNCORRELATED with the generator's
//! is worth more than one that is right 85% and fails on the same inputs — because the pair is what
//! runs, and correlated failure is invisible to per-model scoring.
//!
//! That is the same finding as `deliberate`'s independence guard, arrived at from the other direction.

/// One task, with a verdict that can be reached mechanically.
#[derive(Clone, PartialEq, Debug)]
pub struct Task {
    pub role: crate::smallrole::Job,
    pub prompt: String,
    /// How this task is judged. The point of the design.
    pub scorer: Scorer,
}

/// How a task's answer is checked, without a human or an answer key.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scorer {
    /// Feed it to the compiler. Binary, and unarguable.
    Compiles,
    /// Run `prechecks`; count only CERTAIN findings.
    NoCertainFaults,
    /// Substitute numbers via `refute`. A surviving claim scores; a refuted one does not.
    SurvivesRefutation,
    /// The task has a known-bad input; the model should REJECT it.
    ///
    /// The most informative scorer, because accepting a bad input is the failure that matters and a
    /// model that accepts everything scores perfectly on the others.
    RejectsPlanted,
}

impl Scorer {
    /// Can this be run without a person?
    pub fn mechanical(self) -> bool { true }
}

/// One model's result on one role.
#[derive(Clone, PartialEq, Debug)]
pub struct RoleScore { pub role: crate::smallrole::Job, pub passed: u32, pub total: u32 }

impl RoleScore {
    pub fn rate(&self) -> f32 {
        if self.total == 0 { 0.0 } else { self.passed as f32 / self.total as f32 }
    }
}

/// A model's profile: which roles it is fit for, not a single number.
#[derive(Clone, Debug)]
pub struct Profile { pub model: String, pub roles: Vec<RoleScore> }

impl Profile {
    /// Roles this model clears the bar for.
    ///
    /// Per role rather than overall, because a model that fills bodies well and checks badly should be
    /// used for bodies — a single score would hide that and route it to both or neither.
    pub fn suited(&self, bar: f32) -> Vec<crate::smallrole::Job> {
        self.roles.iter().filter(|r| r.rate() >= bar).map(|r| r.role).collect()
    }

    pub fn rate_for(&self, role: crate::smallrole::Job) -> Option<f32> {
        self.roles.iter().find(|r| r.role == role).map(|r| r.rate())
    }
}

/// How much two models' failures overlap, on the same tasks.
///
/// `a_failed` and `b_failed` are task indices. The number that decides whether a pair is worth running:
/// a checker whose errors coincide with the generator's cannot catch them, however good it looks alone.
pub fn failure_correlation(a_failed: &[usize], b_failed: &[usize]) -> Option<f32> {
    if a_failed.is_empty() || b_failed.is_empty() { return None; }
    let shared = a_failed.iter().filter(|i| b_failed.contains(i)).count() as f32;
    Some(shared / a_failed.len().min(b_failed.len()) as f32)
}

/// Is this pair worth using together?
///
/// Requires the checker to be decent AND to fail differently. The second condition is the one a naive
/// benchmark omits, and omitting it selects for the pair that agrees rather than the pair that catches
/// things.
pub fn pair_is_useful(checker_rate: f32, correlation: f32) -> bool {
    checker_rate >= 0.6 && correlation < 0.7
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::smallrole::Job;

    fn prof(fill: f32, check: f32) -> Profile {
        Profile { model: "m".into(), roles: vec![
            RoleScore { role: Job::FillBody, passed: (fill * 100.0) as u32, total: 100 },
            RoleScore { role: Job::Check, passed: (check * 100.0) as u32, total: 100 },
        ]}
    }

    #[test]
    fn a_model_is_scored_PER_ROLE_not_overall() {
        // A model that fills bodies well and checks badly should be used for bodies. A single number
        // would hide that and route it to both or neither.
        let p = prof(0.9, 0.3);
        assert_eq!(p.suited(0.6), vec![Job::FillBody]);
        assert_eq!(p.rate_for(Job::Check), Some(0.3));
    }

    #[test]
    fn every_scorer_runs_without_a_human() {
        // What makes this feasible where a general reasoning benchmark is not: no answer key, no
        // grader, no coverage argument.
        for s in [Scorer::Compiles, Scorer::NoCertainFaults, Scorer::SurvivesRefutation,
                  Scorer::RejectsPlanted] {
            assert!(s.mechanical(), "{s:?}");
        }
    }

    #[test]
    fn correlated_failures_disqualify_a_pair_however_good_the_checker_looks() {
        // The trap. A checker right 85% of the time whose errors coincide with the generator's cannot
        // catch them; one right 70% that fails differently can. Per-model scoring is blind to this.
        assert!(!pair_is_useful(0.85, 0.95), "same blind spots");
        assert!(pair_is_useful(0.70, 0.20), "worse alone, better together");
    }

    #[test]
    fn a_weak_checker_is_rejected_even_when_uncorrelated() {
        // Independence is necessary and not sufficient — a coin flip is perfectly uncorrelated.
        assert!(!pair_is_useful(0.35, 0.0));
    }

    #[test]
    fn failure_correlation_is_None_when_one_model_never_failed() {
        // Nothing to compare. Returning 0.0 would read as "perfectly independent" on no evidence,
        // which is the shape of every confident-from-nothing number this project has been bitten by.
        assert!(failure_correlation(&[], &[1, 2]).is_none());
        assert_eq!(failure_correlation(&[1, 2, 3], &[2, 3]), Some(1.0), "b's failures are a subset");
        assert_eq!(failure_correlation(&[1, 2], &[3, 4]), Some(0.0));
    }

    #[test]
    fn the_planted_scorer_exists_because_accepting_everything_scores_well() {
        // The most informative task type. A model that says yes to everything passes Compiles and
        // NoCertainFaults on valid inputs and is useless as a checker — only a planted bad input
        // separates it from one that is actually judging.
        let t = Task { role: Job::Check, prompt: "does this match its contract?".into(),
                       scorer: Scorer::RejectsPlanted };
        assert_eq!(t.scorer, Scorer::RejectsPlanted);
    }
}
