//! buildpolicy.rs — what the coordinator does automatically, and when a failed build is worth rerunning.
//!
//! Two questions, and the second matters more.
//!
//! ## What runs by default
//!
//! Verifying the build should be on wherever the loop can act on the result — stepwise and patch modes
//! produce a compilable artifact at every step, so a build is the cheapest real signal available and
//! withholding it just means finding the same error later. Fetching dependencies is different: it touches
//! the NETWORK, and `autonomy::Reach` records a deliberate invariant that no core capability requires it.
//! So fetch defaults on only in aggressive mode, and even there it DEGRADES rather than fails — a machine
//! with no network builds against what is vendored and says so, instead of stopping.
//!
//! ## When to rerun a failed build
//!
//! Rerunning an identical failure is pointless. The interesting rule is not "retry N times" but "retry
//! only while something is CHANGING": a build whose failure signature is unchanged has told you
//! everything it is going to tell you, and the next attempt will burn a model call to produce the same
//! diagnostic. So the ledger keys on a normalized signature and stops the moment it repeats — which also
//! catches the common failure where a model "fixes" an error by moving it.

/// How much the coordinator does without being asked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Nothing automatic. The operator drives.
    Manual,
    /// One bounded piece at a time, each verified before the next.
    Stepwise,
    /// Applying diffs to existing files.
    Patch,
    /// Fan out, verify, repair, repeat — with the widest latitude.
    Aggressive,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Policy {
    /// Compile after each change.
    pub verify_build: bool,
    /// Allow fetching dependencies. Network-touching; see the module note.
    pub fetch_deps: bool,
    /// How many reruns a single failing build may have, GIVEN that each shows a new failure.
    pub retry_budget: usize,
    /// Run the deterministic pre-checks before spending a compile.
    pub prechecks: bool,
}

impl Policy {
    pub fn for_mode(m: Mode) -> Self {
        match m {
            // Manual keeps every side effect off: someone chose this because they want to watch.
            Mode::Manual => Policy { verify_build: false, fetch_deps: false, retry_budget: 0, prechecks: true },
            // Stepwise and Patch both produce a compilable artifact at every step, so building is the
            // cheapest signal available and not building it just defers the same error. They retry, but
            // modestly: the operator is present and watching each step.
            Mode::Stepwise => Policy { verify_build: true, fetch_deps: false, retry_budget: 2, prechecks: true },
            Mode::Patch => Policy { verify_build: true, fetch_deps: false, retry_budget: 2, prechecks: true },
            // Aggressive is the only mode that fetches, because it is the only one where nobody is
            // waiting to approve it.
            Mode::Aggressive => Policy { verify_build: true, fetch_deps: true, retry_budget: 4, prechecks: true },
        }
    }

    /// The policy actually applied, given whether the network is reachable.
    ///
    /// Degrades rather than fails: an offline machine builds against what is vendored. The invariant in
    /// `autonomy::Reach` is that no core capability REQUIRES the network, and a coordinator that stopped
    /// dead without one would break it.
    pub fn resolved(self, network_available: bool) -> (Policy, Option<&'static str>) {
        if self.fetch_deps && !network_available {
            (Policy { fetch_deps: false, ..self },
             Some("no network: building against vendored dependencies only. A missing crate will fail \
the build rather than be fetched."))
        } else { (self, None) }
    }
}

/// A build attempt's outcome, reduced to what decides the next move.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Attempt {
    Passed,
    Failed { signature: String },
}

/// What to do after a failed build.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Retry {
    /// Try again; `0` is how many attempts remain after this one.
    Go(usize),
    /// The same failure as last time. Another attempt will produce the same diagnostic.
    SameFailure(String),
    /// Budget spent.
    Exhausted(usize),
}

impl Retry {
    pub fn should_retry(&self) -> bool { matches!(self, Retry::Go(_)) }
    pub fn explain(&self) -> String {
        match self {
            Retry::Go(n) => format!("retrying ({n} attempts left after this)"),
            Retry::SameFailure(s) => format!(
                "stopping: the build failed the same way twice ({s}). Another round would produce the \
same diagnostic — this needs a different approach, not another attempt."),
            Retry::Exhausted(n) => format!("stopping: {n} rebuild attempts used"),
        }
    }
}

/// Normalize compiler output so the SAME error at a shifted line reads as the same failure.
///
/// Keeps error codes and message text; drops line/column numbers, absolute paths and the arrow/caret
/// diagnostic art. Without this, inserting one line above an error changes the signature and the ledger
/// would happily loop forever on a fault that never moved.
pub fn signature(compiler_output: &str) -> String {
    let mut keys: Vec<String> = Vec::new();
    for line in compiler_output.lines() {
        let l = line.trim();
        if !(l.starts_with("error") || l.starts_with("warning: unused")) { continue; }
        // strip a trailing `--> path:line:col` if present on the same line
        let core = l.split(" --> ").next().unwrap_or(l);
        // drop digits so `expected 2 fields, found 0` and line numbers do not both perturb it
        let norm: String = core.chars().map(|c| if c.is_ascii_digit() { '#' } else { c }).collect();
        if !keys.contains(&norm) { keys.push(norm); }
    }
    if keys.is_empty() { return "unrecognized-failure".into(); }
    keys.sort();
    keys.join(" | ")
}

/// Tracks reruns for one unit of work.
#[derive(Clone, Debug, Default)]
pub struct RetryLedger {
    seen: Vec<String>,
    used: usize,
}

impl RetryLedger {
    pub fn new() -> Self { Self::default() }
    pub fn attempts(&self) -> usize { self.used }

    /// Record a failure and decide whether to rerun.
    pub fn record(&mut self, attempt: &Attempt, policy: &Policy) -> Retry {
        let Attempt::Failed { signature } = attempt else {
            // A pass clears the history: a later, different failure deserves its own budget rather than
            // inheriting exhaustion from an unrelated earlier one.
            self.seen.clear();
            self.used = 0;
            return Retry::Exhausted(0);
        };
        self.used += 1;
        if self.seen.contains(signature) {
            return Retry::SameFailure(signature.clone());
        }
        self.seen.push(signature.clone());
        if self.used > policy.retry_budget { return Retry::Exhausted(policy.retry_budget); }
        Retry::Go(policy.retry_budget - self.used)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stepwise_and_patch_build_by_default_because_they_can_act_on_it() {
        for m in [Mode::Stepwise, Mode::Patch] {
            let p = Policy::for_mode(m);
            assert!(p.verify_build, "{m:?} produces a compilable artifact at every step");
            assert!(p.retry_budget > 0, "{m:?} should rerun a changed failure");
            assert!(!p.fetch_deps, "{m:?} has an operator present; do not touch the network unasked");
        }
        let man = Policy::for_mode(Mode::Manual);
        assert!(!man.verify_build && man.retry_budget == 0, "manual means manual");
        assert!(man.prechecks, "the free deterministic check stays on everywhere");
    }

    #[test]
    fn only_aggressive_fetches_and_it_degrades_rather_than_fails() {
        let a = Policy::for_mode(Mode::Aggressive);
        assert!(a.fetch_deps && a.verify_build);
        assert!(a.retry_budget > Policy::for_mode(Mode::Stepwise).retry_budget);

        // The Reach invariant: no core capability REQUIRES the network. Offline must degrade.
        let (p, note) = a.resolved(false);
        assert!(!p.fetch_deps);
        assert!(note.unwrap().contains("vendored"), "it must say what it will do instead");
        assert_eq!(a.resolved(true), (a, None), "online is unchanged and unremarked");
    }

    #[test]
    fn the_same_failure_twice_stops_the_loop() {
        // This is the rule that matters: a build that failed identically has told you everything it is
        // going to. Another round burns a model call to reproduce the same diagnostic.
        let p = Policy::for_mode(Mode::Aggressive);
        let mut l = RetryLedger::new();
        let f = Attempt::Failed { signature: "error[E0308]: mismatched types".into() };
        assert_eq!(l.record(&f, &p), Retry::Go(3));
        match l.record(&f, &p) {
            Retry::SameFailure(s) => {
                assert!(s.contains("E0308"));
                assert!(Retry::SameFailure(s).explain().contains("different approach, not another attempt"));
            }
            other => panic!("expected SameFailure, got {other:?}"),
        }
    }

    #[test]
    fn a_changed_failure_keeps_going_until_the_budget() {
        let p = Policy::for_mode(Mode::Stepwise); // budget 2
        let mut l = RetryLedger::new();
        let mk = |s: &str| Attempt::Failed { signature: s.into() };
        assert_eq!(l.record(&mk("a"), &p), Retry::Go(1));
        assert_eq!(l.record(&mk("b"), &p), Retry::Go(0));
        assert_eq!(l.record(&mk("c"), &p), Retry::Exhausted(2), "progress does not buy unlimited rounds");
    }

    #[test]
    fn a_pass_clears_the_history_so_later_faults_get_their_own_budget() {
        let p = Policy::for_mode(Mode::Stepwise);
        let mut l = RetryLedger::new();
        l.record(&Attempt::Failed { signature: "a".into() }, &p);
        l.record(&Attempt::Passed, &p);
        assert_eq!(l.attempts(), 0);
        // the same signature is now allowed again — it is a NEW fault in new code, not a repeat
        assert_eq!(l.record(&Attempt::Failed { signature: "a".into() }, &p), Retry::Go(1));
    }

    #[test]
    fn line_numbers_do_not_change_the_signature() {
        // Inserting one line above an error would otherwise mint a fresh signature every round and the
        // ledger would loop forever on a fault that never moved.
        let a = "error[E0433]: failed to resolve: use of undeclared crate\n --> src/main.rs:12:5\n";
        let b = "error[E0433]: failed to resolve: use of undeclared crate\n --> src/main.rs:47:9\n";
        assert_eq!(signature(a), signature(b));
    }

    #[test]
    fn different_errors_produce_different_signatures() {
        let a = "error[E0308]: mismatched types";
        let b = "error[E0599]: no method named `next` found";
        assert_ne!(signature(a), signature(b));
        // and order of the same two errors does not matter — the compiler may reorder them
        let ab = "error[E0308]: mismatched types\nerror[E0599]: no method named `next` found";
        let ba = "error[E0599]: no method named `next` found\nerror[E0308]: mismatched types";
        assert_eq!(signature(ab), signature(ba));
    }

    #[test]
    fn output_with_no_recognizable_error_is_named_not_guessed() {
        // A signature of "" would compare equal to every other empty one and stop the loop on the first
        // unparseable output.
        assert_eq!(signature("Compiling foo v0.1.0\nBuilding [===>   ]"), "unrecognized-failure");
    }
}
