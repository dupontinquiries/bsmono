//! benchartifact.rs — what a benchmark run must capture for its numbers to mean anything.
//!
//! Every measurement in this project's recent history was uninterpretable until paired with its
//! environment. A few from one session:
//!
//! * gates green on lavapipe and red on Honeykrisp, because one divides correctly and one does not
//! * "levers OFF, cached" — a verdict from a build where the levers were unreachable
//! * "1 poll, 141 downloads" — two counters measuring different things under one name
//! * a stale `autotune-v3.tsv` serving verdicts from before a kernel fix
//! * GPU timeouts in `journalctl` that never appeared in the application log at all
//!
//! **A tok/s figure without its environment is a number, not a result.** This is the manifest that
//! makes one comparable to another.
//!
//! ## What must be captured, and why each earns its place
//!
//! Not a shopping list — every field below corresponds to something that has already made a
//! measurement wrong.

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Environment {
    /// Adapter string. lavapipe and Honeykrisp disagree about arithmetic; a result is meaningless
    /// without knowing which produced it.
    pub adapter: String,
    /// Commit of the tree under test. Four fixes in this session produced byte-identical output, and
    /// only the commit distinguishes those runs.
    pub commit: String,
    /// Whether persisted autotune and A/B caches were cleared. A cached verdict outlives the code
    /// that produced it, so an uncleared run may be measuring a deleted build.
    pub caches_cleared: bool,
    /// Environment overrides in force. "levers OFF" meant something different once
    /// LOGSCOPE_MATVEC_SG was readable.
    pub levers: Vec<String>,
    /// GPU timeouts from the kernel log during the run. These have never appeared in the application
    /// log, and a run with five of them is not the same run as one with none.
    pub gpu_timeouts: u32,
    /// Concurrent GPU accessors. The whole point of comparing bare against libkrun — and a figure
    /// taken under contention is not comparable to one taken alone.
    pub accessors: u32,
    /// Isolation used, if any.
    pub isolation: Isolation,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Isolation { None, Libkrun, Container }

/// Why two runs cannot be compared.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Incomparable {
    DifferentAdapter { a: String, b: String },
    DifferentCommit { a: String, b: String },
    StaleCaches,
    /// One run hit GPU timeouts and the other did not.
    TimeoutSkew { a: u32, b: u32 },
    /// Different numbers of concurrent accessors — the thing being measured, if deliberate, and a
    /// confound if not.
    ContentionSkew { a: u32, b: u32 },
}

impl Incomparable {
    pub fn explain(&self) -> String {
        match self {
            Incomparable::DifferentAdapter { a, b } => format!(
                "different adapters ({a} vs {b}) — these disagree about float division and rounding, \
so any quantization figure is measuring the driver rather than the change"),
            Incomparable::DifferentCommit { a, b } => format!(
                "different commits ({a} vs {b}) — four fixes in one session produced byte-identical \
output, and only the commit tells those runs apart"),
            Incomparable::StaleCaches =>
                "one run did not clear its autotune and A/B caches, so it may be reporting verdicts \
from a build that no longer exists".to_string(),
            Incomparable::TimeoutSkew { a, b } => format!(
                "{a} GPU timeouts against {b} — a run whose dispatches were killed is not a slower \
version of a run whose were not, it is a different run"),
            Incomparable::ContentionSkew { a, b } => format!(
                "{a} concurrent accessors against {b} — fine if contention IS the comparison, a \
confound if it is not, and the manifest cannot tell which"),
        }
    }
}

/// Can these two runs be compared at all?
///
/// **Returns reasons rather than a bool**, because a caller comparing bare against libkrun EXPECTS a
/// contention difference and should be able to accept that one while still being told about the
/// adapter mismatch it did not intend.
pub fn comparability(a: &Environment, b: &Environment) -> Vec<Incomparable> {
    let mut v = Vec::new();
    if a.adapter != b.adapter {
        v.push(Incomparable::DifferentAdapter { a: a.adapter.clone(), b: b.adapter.clone() });
    }
    if a.commit != b.commit {
        v.push(Incomparable::DifferentCommit { a: a.commit.clone(), b: b.commit.clone() });
    }
    if !a.caches_cleared || !b.caches_cleared { v.push(Incomparable::StaleCaches); }
    if (a.gpu_timeouts == 0) != (b.gpu_timeouts == 0) {
        v.push(Incomparable::TimeoutSkew { a: a.gpu_timeouts, b: b.gpu_timeouts });
    }
    if a.accessors != b.accessors {
        v.push(Incomparable::ContentionSkew { a: a.accessors, b: b.accessors });
    }
    v
}

/// Files a run must produce. A benchmark whose artifact lacks any of these produced a number nobody
/// can check.
pub const REQUIRED_ARTIFACTS: [&str; 5] = [
    "logscope.log",        // the application's own account
    "journalctl.log",      // GPU timeouts, which never reach the application log
    "manifest.json",       // the Environment above
    "gates.log",           // which gates passed on this hardware at this commit
    "phases.json",         // the sync/compute/submit split per generation
];

#[cfg(test)]
mod tests {
    use super::*;

    fn env(adapter: &str, commit: &str, cleared: bool, timeouts: u32, acc: u32) -> Environment {
        Environment { adapter: adapter.into(), commit: commit.into(), caches_cleared: cleared,
                      levers: vec![], gpu_timeouts: timeouts, accessors: acc,
                      isolation: Isolation::None }
    }

    #[test]
    fn LAVAPIPE_against_HONEYKRISP_is_refused_by_name() {
        // The single most misleading comparison available here: one divides correctly and one does
        // not, so a quantization figure across them measures the driver.
        let r = comparability(&env("llvmpipe", "abc", true, 0, 1),
                              &env("Apple M2 Ultra (G14D B1) [Vulkan]", "abc", true, 0, 1));
        assert!(r[0].explain().contains("measuring the driver rather than the change"));
    }

    #[test]
    fn a_run_with_TIMEOUTS_is_not_a_slower_version_of_one_without() {
        // Five timeouts appeared in journalctl during a Scout run and never in the application log.
        // Treating that as "the same run, slower" is how a killed dispatch becomes a perf datapoint.
        let r = comparability(&env("gpu", "abc", true, 5, 1), &env("gpu", "abc", true, 0, 1));
        assert!(r.iter().any(|x| matches!(x, Incomparable::TimeoutSkew { .. })));
        assert!(r[0].explain().contains("it is a different run"));
    }

    #[test]
    fn CONTENTION_is_reported_even_when_it_is_the_point() {
        // Comparing bare against libkrun EXPECTS an accessor difference. It is still reported,
        // because the manifest cannot tell an intended confound from an accidental one and should
        // not pretend to.
        // env(adapter, commit, cleared, TIMEOUTS, ACCESSORS) — my first version varied the timeout
        // slot and asserted "same accessors", which is the argument-order mistake this whole module
        // is about: a fixture that does not describe what it claims.
        let r = comparability(&env("gpu", "abc", true, 0, 4), &env("gpu", "abc", true, 0, 4));
        assert!(r.is_empty(), "same accessors, no contention note");
        let r2 = comparability(&env("gpu", "abc", true, 0, 1), &env("gpu", "abc", true, 0, 4));
        assert!(r2[0].explain().contains("a confound if it is not"));
    }

    #[test]
    fn STALE_CACHES_invalidate_a_run_even_when_everything_else_matches() {
        // A cached verdict outlives the code that produced it — an autotune-v3.tsv served pre-fix
        // tile choices through an entire kernel rewrite.
        let r = comparability(&env("gpu", "abc", true, 0, 1), &env("gpu", "abc", false, 0, 1));
        assert!(r.iter().any(|x| matches!(x, Incomparable::StaleCaches)));
    }

    #[test]
    fn identical_environments_compare_CLEANLY() {
        assert!(comparability(&env("gpu", "abc", true, 0, 2), &env("gpu", "abc", true, 0, 2)).is_empty());
    }

    #[test]
    fn REASONS_are_returned_rather_than_a_bool() {
        // A caller comparing isolation modes needs to accept the contention difference while still
        // hearing about the adapter mismatch it did not intend. A bool collapses those.
        let r = comparability(&env("llvmpipe", "abc", true, 0, 1), &env("gpu", "def", false, 3, 4));
        assert!(r.len() >= 4, "every reason, not the first: {r:?}");
    }

    #[test]
    fn the_artifact_list_includes_JOURNALCTL() {
        // The one source that has never appeared in the application log, and the one that explained
        // a hang the log recorded as silence.
        assert!(REQUIRED_ARTIFACTS.contains(&"journalctl.log"));
        assert!(REQUIRED_ARTIFACTS.contains(&"manifest.json"));
    }
}
