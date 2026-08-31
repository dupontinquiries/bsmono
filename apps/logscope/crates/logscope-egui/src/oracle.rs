//! oracle — deciding whether a change is CORRECT, deterministically, with no second model and no
//! human looking at it.
//!
//! ## What the verify gate could not tell you
//!
//! The loop's evidence has been `cargo check`/`cargo test` exit status. That answers "does it
//! build" and "does the suite pass", and neither is the question. A green suite after a change is
//! consistent with three completely different worlds:
//!
//!   1. the change fixed something a test was catching,
//!   2. the change did nothing at all and the suite was already green,
//!   3. the change broke something no test covers.
//!
//! An agent that treats (2) as success will happily spend forty turns producing edits that change
//! no observable behaviour and report every one of them as verified. That is the failure this
//! module exists to make impossible to state.
//!
//! ## The three deterministic oracles, in order of strength
//!
//! **1. Status delta.** Run the suite before and after and compare per-test outcomes. A test that
//! went red→green is evidence the change did something the repo already knew how to check. A suite
//! that is byte-identical before and after is evidence of nothing — reported as `Unproven`, not as
//! success. This is cheap and needs only what the repo already has.
//!
//! **2. Inverse-patch (sabotage).** For a change that ADDS a test: revert the source change, keep
//! the test, and re-run. If the new test still passes, it does not test the change — it is vacuous,
//! and a vacuous test is worse than no test because it reports as coverage forever. This is the
//! discipline that caught four of my own wrong tests over one session by hand; there is no reason
//! it should be manual.
//!
//! **3. Differential.** Two implementations, one input, compare the bytes. This is the strongest
//! oracle available and it is nearly free for a PORT: when Rust is replacing Python, the Python IS
//! the specification, and "same stdout on the same input" is a total answer that needs no judgement
//! at all. See [`differential`].
//!
//! ## What this deliberately does not do
//!
//! It does not ask a model whether the change looks right, and it does not score style. Every
//! function here is a comparison of observed outputs — the same discipline as the phase machine's
//! `Checked` signal, one level up: there, evidence decided WHERE the run was; here, evidence decides
//! whether the work was worth anything.

use std::collections::BTreeMap;

/// One test's observed outcome. Ignored tests are recorded as such rather than folded into pass —
/// a suite that "passes" by ignoring the relevant test is exactly the false green this module is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome { Pass, Fail, Ignored }

/// Parse libtest output into per-test outcomes.
///
/// Per-test lines (`test path::name ... ok`) rather than the summary, because the summary counts
/// cannot answer "which test changed". Nextest's `PASS [ 0.1s] crate name` form is accepted too;
/// anything unrecognised is skipped rather than guessed at.
pub fn parse_test_results(raw: &str) -> BTreeMap<String, Outcome> {
    let mut out = BTreeMap::new();
    for line in raw.lines() {
        let t = line.trim();
        // libtest: `test some::path ... ok` / `... FAILED` / `... ignored`
        if let Some(rest) = t.strip_prefix("test ") {
            if let Some((name, tail)) = rest.rsplit_once(" ... ") {
                let name = name.trim();
                // `test result: ok. …` is the summary line, not a test called "result:".
                if name.is_empty() || name.starts_with("result:") { continue; }
                let o = match tail.trim() {
                    x if x.starts_with("ok") => Outcome::Pass,
                    x if x.starts_with("FAILED") => Outcome::Fail,
                    x if x.starts_with("ignored") => Outcome::Ignored,
                    _ => continue,
                };
                out.insert(name.to_string(), o);
            }
            continue;
        }
        // nextest: `        PASS [   0.014s] crate-name test::path`
        for (tag, o) in [("PASS", Outcome::Pass), ("FAIL", Outcome::Fail)] {
            if let Some(rest) = t.strip_prefix(tag) {
                if let Some(close) = rest.find(']') {
                    let name = rest[close + 1..].trim();
                    if !name.is_empty() { out.insert(name.to_string(), o); }
                }
            }
        }
    }
    out
}

/// What changed between two runs of the same suite.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Delta {
    /// Failed before, passes now. The change did something the repo could already check.
    pub fixed: Vec<String>,
    /// Passed before, fails now. A regression, whatever else the change achieved.
    pub broken: Vec<String>,
    /// Did not exist before and passes now. Only meaningful once proven non-vacuous — see
    /// [`vacuous`].
    pub added_passing: Vec<String>,
    /// Did not exist before and fails now.
    pub added_failing: Vec<String>,
    /// Existed before and no longer runs. A test deleted to make a suite green is the oldest trick
    /// there is, so it is surfaced rather than silently dropped from the comparison.
    pub disappeared: Vec<String>,
}

impl Delta {
    pub fn is_empty(&self) -> bool {
        self.fixed.is_empty() && self.broken.is_empty() && self.added_passing.is_empty()
            && self.added_failing.is_empty() && self.disappeared.is_empty()
    }
}

pub fn delta(before: &BTreeMap<String, Outcome>, after: &BTreeMap<String, Outcome>) -> Delta {
    let mut d = Delta::default();
    for (name, &now) in after {
        match before.get(name) {
            Some(&was) => {
                if was == Outcome::Fail && now == Outcome::Pass { d.fixed.push(name.clone()); }
                if was == Outcome::Pass && now == Outcome::Fail { d.broken.push(name.clone()); }
            }
            None => match now {
                Outcome::Pass => d.added_passing.push(name.clone()),
                Outcome::Fail => d.added_failing.push(name.clone()),
                Outcome::Ignored => {}
            },
        }
    }
    for name in before.keys() {
        if !after.contains_key(name) { d.disappeared.push(name.clone()); }
    }
    d
}

/// The verdict on a change, from evidence alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Evidence {
    /// A test went red→green, or a genuinely new test passes. The change is observable.
    Proven(String),
    /// Something that passed now fails, or a test vanished.
    Regressed(String),
    /// The suite cannot tell this change from no change. NOT success — the honest answer when a
    /// green run is the same green run as before.
    Unproven(String),
}

/// Judge a delta. `new_tests_vacuous` are tests the change added that ALSO pass without the change
/// (see [`vacuous`]); they are excluded from counting as proof, because they prove nothing.
pub fn judge(d: &Delta, new_tests_vacuous: &[String]) -> Evidence {
    if !d.broken.is_empty() {
        return Evidence::Regressed(format!(
            "{} test(s) that passed before now fail: {}", d.broken.len(), preview(&d.broken)));
    }
    if !d.disappeared.is_empty() {
        return Evidence::Regressed(format!(
            "{} test(s) no longer run: {}. A suite made green by removing a test is not a green suite.",
            d.disappeared.len(), preview(&d.disappeared)));
    }
    if !d.added_failing.is_empty() {
        return Evidence::Unproven(format!(
            "{} newly added test(s) fail: {}", d.added_failing.len(), preview(&d.added_failing)));
    }
    let real_new: Vec<&String> = d.added_passing.iter()
        .filter(|n| !new_tests_vacuous.iter().any(|v| v == *n)).collect();
    if !d.fixed.is_empty() {
        return Evidence::Proven(format!(
            "{} test(s) went from failing to passing: {}", d.fixed.len(), preview(&d.fixed)));
    }
    if !real_new.is_empty() {
        return Evidence::Proven(format!(
            "{} new test(s) pass and fail without the change: {}",
            real_new.len(), preview(&real_new.iter().map(|s| (*s).clone()).collect::<Vec<_>>())));
    }
    if !new_tests_vacuous.is_empty() {
        return Evidence::Unproven(format!(
            "{} test(s) were added and pass WITHOUT the change too: {}. They do not test it.",
            new_tests_vacuous.len(), preview(new_tests_vacuous)));
    }
    Evidence::Unproven(
        "no test changed status. The suite cannot tell this change from no change — either it is a \
no-op, or nothing here covers what it altered.".into())
}

fn preview(v: &[String]) -> String {
    let shown: Vec<&str> = v.iter().take(4).map(|s| s.as_str()).collect();
    format!("{}{}", shown.join(", "), if v.len() > shown.len() { ", …" } else { "" })
}

/// Which of the change's new tests are VACUOUS: present in `after`, passing in `after`, and also
/// passing in `reverted` — the tree with the source change undone but the tests kept.
///
/// A test that passes with and without the change does not test the change. It is worse than no
/// test, because it reports as coverage of that behaviour forever.
pub fn vacuous(after: &BTreeMap<String, Outcome>, before: &BTreeMap<String, Outcome>,
               reverted: &BTreeMap<String, Outcome>) -> Vec<String> {
    after.iter()
        .filter(|(n, o)| **o == Outcome::Pass && !before.contains_key(*n))
        .filter(|(n, _)| reverted.get(*n) == Some(&Outcome::Pass))
        .map(|(n, _)| n.clone())
        .collect()
}

// ── differential: the port oracle ───────────────────────────────────────────────────────────────

/// One input, two implementations, compared byte for byte.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Diff {
    Same,
    /// Differ, with the first differing line reported by 1-based line number.
    Differ { line: usize, reference: String, candidate: String },
}

/// Compare a reference implementation's output against a candidate's.
///
/// THE STRONGEST ORACLE THERE IS, and nearly free for a port. When Rust is replacing Python, the
/// Python is the specification: "same bytes on the same input" needs no judgement, no model and no
/// reviewer. For a python → pylower → rust pipeline this is the whole verification story — every
/// lowered function is checkable against the function it was lowered from.
///
/// Reported by line rather than as a bool because "they differ" sends a model back to re-read both
/// implementations, while "line 40: expected 3.14159, got 3.14158" points at the bug. Trailing
/// whitespace on a line is ignored (it is a formatting difference, not a behavioural one) but a
/// differing number of lines is not.
pub fn differential(reference: &str, candidate: &str) -> Diff {
    let (r, c): (Vec<&str>, Vec<&str>) = (reference.lines().collect(), candidate.lines().collect());
    for i in 0..r.len().max(c.len()) {
        let (a, b) = (r.get(i).unwrap_or(&"").trim_end(), c.get(i).unwrap_or(&"").trim_end());
        if a != b {
            return Diff::Differ {
                line: i + 1,
                reference: a.chars().take(200).collect(),
                candidate: b.chars().take(200).collect(),
            };
        }
    }
    Diff::Same
}

/// The observation text a differential result folds into the model's next turn.
pub fn differential_report(cmd_ref: &str, cmd_cand: &str, d: &Diff) -> String {
    match d {
        Diff::Same => format!(
            "DIFFERENTIAL: `{cmd_cand}` matches `{cmd_ref}` byte for byte on this input. That is the \
strongest evidence available for a port — the reference implementation IS the specification."),
        Diff::Differ { line, reference, candidate } => format!(
            "DIFFERENTIAL: `{cmd_cand}` diverges from `{cmd_ref}` at line {line}.\n  \
reference: {reference}\n  candidate: {candidate}\nFix the candidate to match; do not adjust the \
reference to agree with it."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn results(pairs: &[(&str, Outcome)]) -> BTreeMap<String, Outcome> {
        pairs.iter().map(|(n, o)| (n.to_string(), *o)).collect()
    }

    #[test]
    fn libtest_output_parses_per_test_not_per_summary() {
        let raw = "\nrunning 4 tests\ntest a::one ... ok\ntest a::two ... FAILED\ntest b::three ... ignored\n\
test b::four ... ok\n\ntest result: FAILED. 2 passed; 1 failed; 1 ignored; 0 measured\n";
        let r = parse_test_results(raw);
        assert_eq!(r.len(), 4, "the summary line is not a test: {r:?}");
        assert_eq!(r["a::one"], Outcome::Pass);
        assert_eq!(r["a::two"], Outcome::Fail);
        assert_eq!(r["b::three"], Outcome::Ignored);
    }

    #[test]
    fn nextest_output_parses_too() {
        let raw = "        PASS [   0.014s] mycrate lib::tests::works\n        FAIL [   0.002s] mycrate lib::tests::broken\n";
        let r = parse_test_results(raw);
        assert_eq!(r["mycrate lib::tests::works"], Outcome::Pass);
        assert_eq!(r["mycrate lib::tests::broken"], Outcome::Fail);
    }

    #[test]
    fn a_green_suite_that_was_already_green_proves_nothing() {
        let before = results(&[("a", Outcome::Pass), ("b", Outcome::Pass)]);
        let after = before.clone();
        let d = delta(&before, &after);
        assert!(d.is_empty());
        match judge(&d, &[]) {
            Evidence::Unproven(why) => assert!(why.contains("cannot tell this change from no change"), "{why}"),
            other => panic!("a green-to-green run is not proof: {other:?}"),
        }
    }

    #[test]
    fn a_test_going_red_to_green_is_proof() {
        let before = results(&[("a", Outcome::Fail), ("b", Outcome::Pass)]);
        let after = results(&[("a", Outcome::Pass), ("b", Outcome::Pass)]);
        match judge(&delta(&before, &after), &[]) {
            Evidence::Proven(why) => assert!(why.contains("failing to passing") && why.contains('a'), "{why}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_regression_outranks_whatever_else_the_change_achieved() {
        // fixed one, broke another: still a regression.
        let before = results(&[("a", Outcome::Fail), ("b", Outcome::Pass)]);
        let after = results(&[("a", Outcome::Pass), ("b", Outcome::Fail)]);
        match judge(&delta(&before, &after), &[]) {
            Evidence::Regressed(why) => assert!(why.contains('b'), "{why}"),
            other => panic!("{other:?}"),
        }
    }

    /// The oldest trick: delete the failing test.
    #[test]
    fn a_test_that_vanished_is_a_regression_not_a_pass() {
        let before = results(&[("a", Outcome::Fail), ("b", Outcome::Pass)]);
        let after = results(&[("b", Outcome::Pass)]);
        let d = delta(&before, &after);
        assert_eq!(d.disappeared, vec!["a".to_string()]);
        match judge(&d, &[]) {
            Evidence::Regressed(why) => assert!(why.contains("no longer run"), "{why}"),
            other => panic!("removing the failing test is not success: {other:?}"),
        }
    }

    /// The inverse-patch check. A test added alongside a change, which passes with the change
    /// reverted, does not test the change.
    #[test]
    fn a_new_test_that_passes_without_the_change_is_vacuous() {
        let before = results(&[("old", Outcome::Pass)]);
        let after = results(&[("old", Outcome::Pass), ("new", Outcome::Pass)]);
        // reverted: source change undone, test kept — and it still passes.
        let reverted = results(&[("old", Outcome::Pass), ("new", Outcome::Pass)]);
        let v = vacuous(&after, &before, &reverted);
        assert_eq!(v, vec!["new".to_string()]);
        match judge(&delta(&before, &after), &v) {
            Evidence::Unproven(why) => assert!(why.contains("WITHOUT the change"), "{why}"),
            other => panic!("a vacuous test is not proof: {other:?}"),
        }
    }

    #[test]
    fn a_new_test_that_fails_without_the_change_is_proof() {
        let before = results(&[("old", Outcome::Pass)]);
        let after = results(&[("old", Outcome::Pass), ("new", Outcome::Pass)]);
        let reverted = results(&[("old", Outcome::Pass), ("new", Outcome::Fail)]);
        let v = vacuous(&after, &before, &reverted);
        assert!(v.is_empty(), "it fails without the change, so it is not vacuous");
        match judge(&delta(&before, &after), &v) {
            Evidence::Proven(why) => assert!(why.contains("fail without the change"), "{why}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_added_test_that_fails_is_unproven_not_regressed() {
        // The change came with a test it does not satisfy. Not a regression (nothing that worked
        // broke), but certainly not done.
        let before = results(&[("old", Outcome::Pass)]);
        let after = results(&[("old", Outcome::Pass), ("new", Outcome::Fail)]);
        match judge(&delta(&before, &after), &[]) {
            Evidence::Unproven(why) => assert!(why.contains("newly added test(s) fail"), "{why}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn differential_points_at_the_first_divergent_line() {
        assert_eq!(differential("a\nb\nc\n", "a\nb\nc\n"), Diff::Same);
        assert_eq!(differential("a\nb  \nc", "a\nb\nc"), Diff::Same, "trailing space is formatting");
        match differential("1\n3.14159\n3", "1\n3.14158\n3") {
            Diff::Differ { line, reference, candidate } => {
                assert_eq!(line, 2);
                assert_eq!(reference, "3.14159");
                assert_eq!(candidate, "3.14158");
            }
            other => panic!("{other:?}"),
        }
        // a missing trailing line is a difference, not formatting
        assert_ne!(differential("a\nb\n", "a\n"), Diff::Same);
    }

    #[test]
    fn the_differential_report_says_which_side_is_the_specification() {
        let d = differential("1\n2\n", "1\n9\n");
        let r = differential_report("python impl.py", "./target/debug/port", &d);
        assert!(r.contains("line 2"));
        assert!(r.contains("do not adjust the reference"),
                "the direction of authority has to be stated or it gets 'fixed' the wrong way: {r}");
        assert!(differential_report("a", "b", &Diff::Same).contains("specification"));
    }
}
