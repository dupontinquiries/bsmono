//! Turning a question into probes, running them, and composing what came back.
//!
//! The piece that makes the rest do anything. `context_probe` says what may be
//! asked, `fan_policy` says how wide, `probe_exec` does the asking; this
//! decides WHAT to ask and stops when the budget is gone.
//!
//! # The decision step that was missing
//!
//! The reported behaviour is that the model opens the first plausible file and
//! answers from it. [`survey_probes`] is the step that never existed: a
//! question becomes a set of cheap survey probes BEFORE any file is opened,
//! and [`read_probes`] turns what those found into reads.
//!
//! Two phases, not one, because the second cannot be planned until the first
//! has answered. That is also why this is a loop rather than a prompt.
//!
//! # The runner is injected
//!
//! `answer` takes a closure rather than calling `probe_exec` directly, so the
//! whole loop is testable with a fake corpus and no filesystem. It is also the
//! seam a worker fan slots into: today the closure runs probes inline, and a
//! fanned version runs `n` of them at once without this file changing.

use crate::context_probe::{estimate_tokens, plan, Budget, Finding, Probe, Refused};

/// What happened, in order. Kept so the operator can see WHY an answer is thin
/// — an empty result and a result cut off by budget look identical otherwise,
/// and lead to different fixes.
#[derive(Debug, Default)]
pub struct Session {
    pub ran: Vec<Probe>,
    pub findings: Vec<Finding>,
    /// Probes that returned nothing or failed, with the reason.
    pub misses: Vec<(Probe, String)>,
    /// Findings the budget refused.
    pub refused: Vec<(Finding, Refused)>,
    pub spent: u32,
    pub budget_exhausted: bool,
}

/// Words worth grepping for, from a question.
///
/// Identifier-shaped tokens only: `snake_case`, `CamelCase`, anything with a
/// `_` or an inner capital, plus explicit paths. Ordinary English is dropped —
/// grepping "the" or "where" returns the repository, and the hit ceiling then
/// spends the whole budget on the first file walked.
pub fn keywords(question: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in question.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.' || c == '/')) {
        let w = raw.trim_matches('.');
        if w.len() < 4 {
            continue;
        }
        let identifier_shaped = w.contains('_')
            || w.contains('/')
            || w.contains('.')
            || w.chars().skip(1).any(|c| c.is_ascii_uppercase());
        if identifier_shaped && !out.iter().any(|e| e == w) {
            out.push(w.to_string());
        }
    }
    out
}

/// Paths named outright in the question.
pub fn mentioned_paths(question: &str) -> Vec<String> {
    keywords(question)
        .into_iter()
        .filter(|w| w.contains('/') || w.ends_with(".rs") || w.ends_with(".toml"))
        .collect()
}

/// Phase one: what to ask before opening anything.
pub fn survey_probes(question: &str) -> Vec<Probe> {
    let mut v = Vec::new();
    for p in mentioned_paths(question) {
        v.push(Probe::Outline { path: p });
    }
    for k in keywords(question) {
        // A path is already covered by its outline; grepping for it as well
        // spends the ceiling re-finding a file we are about to index.
        if k.contains('/') || k.ends_with(".rs") {
            continue;
        }
        v.push(Probe::Grep { pattern: k, glob: Some(".rs".into()), max_hits: 12 });
    }
    v
}

/// How much context to take around a survey hit.
///
/// A grep hit is one line and one line rarely answers anything; a function
/// signature three lines up is usually what makes it legible. Small, because
/// the point is not to end up reading the file by accident.
pub const SLICE_CONTEXT: u32 = 12;

/// Phase two: what the survey justified reading.
///
/// One slice per DISTINCT file, around its first hit. Not one per hit: a
/// keyword appearing forty times in one file would otherwise produce forty
/// overlapping slices and spend the budget re-reading the same region.
pub fn read_probes(found: &[Finding]) -> Vec<Probe> {
    let mut seen: Vec<&str> = Vec::new();
    let mut v = Vec::new();
    for f in found {
        if seen.iter().any(|p| *p == f.path) {
            continue;
        }
        seen.push(&f.path);
        let first = f.first.saturating_sub(SLICE_CONTEXT).max(1);
        v.push(Probe::Slice { path: f.path.clone(), first, last: f.last + SLICE_CONTEXT });
    }
    v
}

/// Run one question to completion, or to the budget.
///
/// `run` executes a probe. Errors are recorded as misses rather than aborting:
/// one bad path must not end a survey that had four other probes to try.
pub fn answer(
    question: &str,
    ceiling: u32,
    mut run: impl FnMut(&Probe) -> Result<Vec<Finding>, String>,
) -> Session {
    let mut s = Session::default();
    let mut budget = Budget::new(ceiling);

    let proposed = survey_probes(question);
    let surveys = plan(&proposed, ceiling);
    // `plan` DROPS what it cannot afford, and a plan that dropped everything
    // is indistinguishable from a question with nothing to look for. Found by
    // a test that set a ceiling too small for a single grep and got back an
    // empty session reporting no problem.
    if surveys.len() < proposed.len() {
        s.budget_exhausted = true;
    }
    let mut survey_hits: Vec<Finding> = Vec::new();
    for p in &surveys {
        match run(p) {
            Ok(f) if f.is_empty() => s.misses.push((p.clone(), "no matches".into())),
            Ok(f) => survey_hits.extend(f),
            Err(e) => s.misses.push((p.clone(), e)),
        }
        s.ran.push(p.clone());
    }

    // Survey findings are admitted first: they are cheap, and they are what
    // makes the reads defensible. Spending the ceiling on reads and then
    // having no room to report what justified them is the wrong way round.
    for f in survey_hits.drain(..) {
        match budget.admit(&f) {
            Ok(_) => s.findings.push(f),
            Err(e) => {
                s.budget_exhausted |= matches!(e, Refused::OverBudget { .. });
                s.refused.push((f, e));
            }
        }
    }

    // Reads are planned from what the survey ADMITTED, so a finding the budget
    // refused cannot justify a slice the budget also cannot afford.
    let reads = read_probes(&s.findings);
    for p in reads {
        if estimate_tokens(&p) > budget.remaining() {
            s.budget_exhausted = true;
            continue;
        }
        match run(&p) {
            Ok(fs) => {
                for f in fs {
                    match budget.admit(&f) {
                        Ok(_) => s.findings.push(f),
                        Err(e) => {
                            s.budget_exhausted |= matches!(e, Refused::OverBudget { .. });
                            s.refused.push((f, e));
                        }
                    }
                }
            }
            Err(e) => s.misses.push((p.clone(), e)),
        }
        s.ran.push(p);
    }

    s.spent = budget.spent();
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(path: &str, line: u32, text: &str) -> Finding {
        Finding { path: path.into(), first: line, last: line, text: text.into() }
    }

    #[test]
    fn ordinary_english_is_not_grepped() {
        // "where is the pane split decided" must not grep "where" and "decided".
        // A common word hits the ceiling in the first file walked and spends
        // the whole budget re-reading it.
        let k = keywords("where is the pane split decided in the editor");
        assert!(k.is_empty(), "grepping plain English: {k:?}");

        // Identifier-shaped words ARE kept.
        let k = keywords("where is split_pane decided in PaneTree");
        assert_eq!(k, vec!["split_pane", "PaneTree"]);
    }

    #[test]
    fn a_named_path_becomes_an_outline_not_a_grep() {
        // The file is about to be indexed; grepping for its name as well
        // spends the ceiling re-finding what we already have.
        let p = survey_probes("what does src/pane_tree.rs do");
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(matches!(&p[0], Probe::Outline { path } if path == "src/pane_tree.rs"), "{p:?}");
    }

    #[test]
    fn nothing_is_read_before_something_is_surveyed() {
        // The reported default behaviour, made impossible. The first phase
        // contains no Slice at all — reads exist only as a consequence of
        // findings.
        let s = answer("find split_pane", 100_000, |_| Ok(vec![]));
        assert!(s.ran.iter().all(|p| !matches!(p, Probe::Slice { .. })),
                "a read ran with no survey findings: {:?}", s.ran);
    }

    #[test]
    fn a_survey_hit_becomes_one_slice_per_file_not_one_per_hit() {
        // Forty hits in one file would otherwise be forty overlapping slices,
        // spending the budget re-reading the same region.
        let hits = vec![f("a.rs", 100, "x"), f("a.rs", 140, "x"), f("b.rs", 5, "x")];
        let r = read_probes(&hits);
        assert_eq!(r.len(), 2, "{r:?}");
        assert!(matches!(&r[0], Probe::Slice { path, .. } if path == "a.rs"));
        assert!(matches!(&r[1], Probe::Slice { path, .. } if path == "b.rs"));
    }

    #[test]
    fn a_slice_near_the_top_of_a_file_does_not_underflow_to_zero() {
        // Line 0 does not exist and `probe_exec` refuses it, so a hit on
        // line 3 with 12 lines of context must clamp rather than produce a
        // slice the executor will reject.
        let r = read_probes(&[f("a.rs", 3, "x")]);
        match &r[0] {
            Probe::Slice { first, .. } => assert_eq!(*first, 1, "{r:?}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn one_failing_probe_does_not_end_the_survey() {
        // A mistyped path must not abandon the four other probes. The miss is
        // RECORDED, because an empty answer and a failed one lead to
        // different fixes.
        let mut n = 0;
        let s = answer("find alpha_one and beta_two", 100_000, |p| {
            n += 1;
            if n == 1 { Err("no such file".into()) } else { Ok(vec![f("b.rs", 9, "hit")]) }
        });
        assert_eq!(s.misses.len(), 1, "{:?}", s.misses);
        assert!(!s.findings.is_empty(), "the survey stopped at the first failure");
    }

    #[test]
    fn the_budget_stops_the_reads_and_says_so() {
        // An answer cut off by budget and an answer with nothing to find look
        // identical unless this is reported, and they need opposite fixes.
        let s = answer("find split_pane", 60, |p| match p {
            Probe::Grep { .. } => Ok(vec![f("a.rs", 50, "hit")]),
            _ => Ok(vec![Finding { path: "a.rs".into(), first: 38, last: 62, text: "big".into() }]),
        });
        assert!(s.budget_exhausted, "the budget ran out silently: {s:?}");
        assert!(s.spent <= 60, "spent {} over a 60 ceiling", s.spent);
    }

    #[test]
    fn a_finding_the_budget_refused_cannot_justify_a_read() {
        // Reads are planned from what was ADMITTED. Planning from everything
        // found would let a refused finding spend budget it was already
        // denied.
        let s = answer("find split_pane", 13, |_| {
            Ok(vec![Finding { path: "a.rs".into(), first: 1, last: 40, text: "x".into() }])
        });
        assert!(s.findings.is_empty(), "an over-budget finding was admitted");
        assert!(s.ran.iter().all(|p| !matches!(p, Probe::Slice { .. })),
                "a refused finding justified a read: {:?}", s.ran);
    }
}
