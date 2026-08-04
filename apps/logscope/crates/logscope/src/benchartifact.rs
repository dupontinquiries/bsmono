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

/// What a driver's arithmetic actually does, measured rather than read from a spec.
///
/// **Every one of these was assumed correctly-behaved and was not**, and each cost a run:
///
/// * division lowered to an approximate reciprocal — broke Q4_K scales
/// * `round()` implementing ties-away where the WGSL spec says ties-to-even
/// * multiply-add contracted in one shader and not another, from identical source
/// * a shift by a lane-varying amount returning zero for three lanes in four
///
/// A benchmark that compares two machines without recording these is comparing arithmetic, not
/// performance. **The spec is not a substitute for the measurement** — reading it is what produced
/// the ties-to-even change that made a gate 8.8x worse.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DriverProfile {
    /// Does `a / b` match a correctly-rounded division?
    pub exact_division: bool,
    /// Does `round()` break ties away from zero (Metal) or to even (the WGSL spec)?
    pub ties_away_from_zero: bool,
    /// Does `a * b + c` contract into an FMA?
    pub contracts_fma: bool,
    /// Does a shift by a lane-varying amount work?
    pub lane_varying_shift: bool,
}

impl DriverProfile {
    /// The profile a spec-reader would predict. **Honeykrisp matches it on none of the four.**
    pub fn by_the_spec() -> Self {
        DriverProfile { exact_division: true, ties_away_from_zero: false,
                        contracts_fma: false, lane_varying_shift: true }
    }

    /// Measured on an M2 Ultra under Asahi/Honeykrisp Vulkan.
    pub fn honeykrisp_measured() -> Self {
        DriverProfile { exact_division: false, ties_away_from_zero: true,
                        contracts_fma: true, lane_varying_shift: false }
    }

    /// Behaviours where this driver departs from what a spec-reader would expect.
    ///
    /// **This is the list a porter needs**, and it is the one nobody writes down because each entry
    /// only becomes visible after it has cost a day.
    pub fn surprises(&self) -> Vec<&'static str> {
        let spec = Self::by_the_spec();
        let mut v = Vec::new();
        if self.exact_division != spec.exact_division {
            v.push("division is an approximate reciprocal — a quotient on a .5 boundary may land \
either side, and no CPU expression reproduces it");
        }
        if self.ties_away_from_zero != spec.ties_away_from_zero {
            v.push("round() breaks ties AWAY from zero, not to even as the WGSL spec states — \
matching the spec here made a gate 8.8x worse");
        }
        if self.contracts_fma != spec.contracts_fma {
            v.push("multiply-add contracts into an FMA, and does so per compilation unit, so \
identical source in two shaders can round differently");
        }
        if self.lane_varying_shift != spec.lane_varying_shift {
            v.push("a shift by a lane-varying amount returns zero for lanes whose shift is non-zero \
— use extractBits, and audit every unpack");
        }
        v
    }

    /// Can a quantization result from this driver be compared bit-for-bit against a CPU reference?
    pub fn allows_bit_exact_quant(&self) -> bool { self.exact_division }
}

#[cfg(test)]
mod driver_tests {
    use super::*;

    #[test]
    fn HONEYKRISP_departs_from_the_spec_on_all_four() {
        // Every one was assumed and every one cost a run. A porter arriving at this driver with the
        // spec in hand gets four wrong answers.
        let p = DriverProfile::honeykrisp_measured();
        assert_eq!(p.surprises().len(), 4, "{:?}", p.surprises());
    }

    #[test]
    fn a_SPEC_CONFORMING_driver_surprises_nobody() {
        assert!(DriverProfile::by_the_spec().surprises().is_empty());
    }

    #[test]
    fn INEXACT_DIVISION_forbids_a_bit_exact_quant_comparison() {
        // The conclusion that took four fixes to reach: a gate demanding bit-equality on this driver
        // is demanding the CPU emulate rcp, which cannot be written.
        assert!(!DriverProfile::honeykrisp_measured().allows_bit_exact_quant());
        assert!(DriverProfile::by_the_spec().allows_bit_exact_quant());
    }

    #[test]
    fn each_surprise_says_what_to_DO_about_it() {
        // "Division is inexact" is a fact; "use extractBits and audit every unpack" is an action, and
        // a porter needs the second.
        let s = DriverProfile::honeykrisp_measured().surprises();
        assert!(s.iter().any(|x| x.contains("use extractBits")));
        assert!(s.iter().any(|x| x.contains("no CPU expression reproduces it")));
    }
}
