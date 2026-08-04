//! buildsignal.rs — turning build and test output into a state marker a run can act on.
//!
//! `longrun::Marker` needs failures and passing counts, and nothing produced them from a real
//! repository. This does — and its job is less "parse cargo output" than **refuse to report a green
//! that was not earned.**
//!
//! Four false greens from one session, each of which cost a run:
//!
//! * a `test result: ok` line read from a run that had raced a concurrent rebuild
//! * `0 tests ran` after a filter matched nothing, reported as passing
//! * gates green on lavapipe for a kernel that was red on the target GPU
//! * a cached autotune verdict describing a build that no longer existed
//!
//! **A parser that only looks for "ok" reproduces all four.**

/// What a build or test invocation actually said.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Signal {
    /// Compiled and every test passed. The count is carried because a green with fewer tests than
    /// last time is a regression wearing a success message.
    Passed { tests: u32 },
    /// Compiled, some tests failed.
    Failed { passed: u32, failed: u32 },
    /// Did not compile. **Distinct from a test failure**: no test result from this run means
    /// anything, and treating it as "0 passed" would let a broken tree look like a fixed one that
    /// happens to fail everything.
    BuildError { errors: u32 },
    /// **Ran nothing.** A filter that matched no tests, or a target that does not exist. Reported
    /// separately because "0 failed" is technically true and completely misleading.
    RanNothing { reason: &'static str },
    /// Output could not be interpreted. Better than guessing: a parser that returns Passed when
    /// confused is the most expensive kind of wrong.
    Unreadable,
}

impl Signal {
    /// Is this a result a run may treat as progress?
    ///
    /// `RanNothing` is deliberately not: it is the shape of every false green here.
    pub fn is_trustworthy_green(&self) -> bool { matches!(self, Signal::Passed { tests } if *tests > 0) }

    pub fn explain(&self) -> String {
        match self {
            Signal::Passed { tests } => format!("{tests} tests passed"),
            Signal::Failed { passed, failed } => format!("{failed} failed, {passed} passed"),
            Signal::BuildError { errors } => format!(
                "{errors} compile errors — no test result from this run means anything, and reading \
one as '0 passed' would make a broken tree look like a fixed one that fails everything"),
            Signal::RanNothing { reason } => format!(
                "ran nothing ({reason}) — '0 failed' is true and misleading, and this is the shape \
of every false green this project has hit"),
            Signal::Unreadable =>
                "could not interpret the output; treating it as green would be the most expensive \
kind of wrong".into(),
        }
    }
}

/// Parse `cargo test` / `cargo build` output.
///
/// Reads the LAST result line rather than the first: a workspace run emits one per target, and the
/// first is rarely the one that failed.
pub fn parse(output: &str) -> Signal {
    let errors = output.lines().filter(|l| l.starts_with("error[") || l.starts_with("error:")).count();
    // A compile error outranks any test line that may also be present from an earlier target.
    if errors > 0 { return Signal::BuildError { errors: errors as u32 } }

    let mut last: Option<Signal> = None;
    for line in output.lines() {
        let Some(rest) = line.strip_prefix("test result: ") else { continue };
        // Take the token immediately BEFORE the keyword. The first segment is "ok. 785 passed", so
        // stripping the suffix leaves "ok. 785" — my first version parsed that and got zero, which
        // then read a healthy run as RanNothing. A false RED rather than a false green, but the same
        // class of parser bug.
        let num = |key: &str| -> u32 {
            let mut prev = "";
            for tok in rest.split_whitespace() {
                let t = tok.trim_end_matches([';', ',', '.']);
                if t == key { return prev.parse().unwrap_or(0) }
                prev = t;
            }
            0
        };
        let (passed, failed) = (num("passed"), num("failed"));
        last = Some(if failed > 0 {
            Signal::Failed { passed, failed }
        } else if passed == 0 {
            // The false green this project keeps hitting: a filter matched nothing and the harness
            // reported success.
            Signal::RanNothing { reason: "0 tests matched" }
        } else {
            Signal::Passed { tests: passed }
        });
    }
    last.unwrap_or(Signal::Unreadable)
}

/// Did the test count DROP between two green runs?
///
/// **A green with fewer tests than before is a regression.** Deleting a failing test turns a red run
/// green, and nothing in a result line distinguishes that from fixing it.
pub fn lost_coverage(before: &Signal, after: &Signal) -> Option<u32> {
    match (before, after) {
        (Signal::Passed { tests: b }, Signal::Passed { tests: a }) if a < b => Some(b - a),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_normal_pass_is_read_with_its_COUNT() {
        assert_eq!(parse("test result: ok. 785 passed; 0 failed; 0 ignored"),
                   Signal::Passed { tests: 785 });
    }

    #[test]
    fn ZERO_TESTS_is_not_a_pass() {
        // The exact shape of a false green: a filter matched nothing and the harness said ok.
        let s = parse("test result: ok. 0 passed; 0 failed; 0 ignored; 552 filtered out");
        assert!(matches!(s, Signal::RanNothing { .. }), "{s:?}");
        assert!(!s.is_trustworthy_green());
        assert!(s.explain().contains("true and misleading"));
    }

    #[test]
    fn a_COMPILE_ERROR_outranks_any_test_line_in_the_same_output() {
        // A workspace run emits results per target; an early target can pass while a later one fails
        // to build. Reading the test line would report the tree as green.
        let s = parse("test result: ok. 10 passed; 0 failed\nerror[E0004]: non-exhaustive patterns");
        assert_eq!(s, Signal::BuildError { errors: 1 });
        assert!(!s.is_trustworthy_green());
    }

    #[test]
    fn a_build_error_is_NOT_reported_as_zero_passed() {
        // Distinct states: a tree that will not compile is not a tree that fails all its tests, and
        // conflating them lets a broken one look like a fixed one.
        assert!(matches!(parse("error: could not compile"), Signal::BuildError { .. }));
    }

    #[test]
    fn the_LAST_result_line_wins() {
        // A workspace emits one per target and the first is rarely the one that failed.
        let s = parse("test result: ok. 10 passed; 0 failed\ntest result: FAILED. 5 passed; 2 failed");
        assert_eq!(s, Signal::Failed { passed: 5, failed: 2 });
    }

    #[test]
    fn UNREADABLE_output_is_not_optimistically_green() {
        // A parser that returns Passed when confused is the most expensive kind of wrong.
        assert_eq!(parse("cargo: command not found"), Signal::Unreadable);
        assert!(!Signal::Unreadable.is_trustworthy_green());
    }

    #[test]
    fn a_green_with_FEWER_TESTS_is_flagged_as_lost_coverage() {
        // Deleting a failing test turns a red run green, and no result line distinguishes that from
        // fixing it.
        let before = Signal::Passed { tests: 785 };
        let after = Signal::Passed { tests: 780 };
        assert_eq!(lost_coverage(&before, &after), Some(5));
        assert_eq!(lost_coverage(&before, &Signal::Passed { tests: 790 }), None);
    }
}
