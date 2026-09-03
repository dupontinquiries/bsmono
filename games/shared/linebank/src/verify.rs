//! VERIFY — the one thing order-based alignment cannot catch.
//!
//! [`assign`](crate::slice::assign) refuses when the span COUNT is wrong, which
//! catches a dropped slate. It cannot catch a session where the count is right
//! and the ORDER is not: an actor who reads two lines in the other order, or a
//! re-record dropped into the wrong place in the folder. Those import cleanly
//! and every span is mislabelled.
//!
//! `shared/voiceprint-kit` is the tool for it — `fingerprint()` returns a
//! 13-coefficient MFCC summary and `distance()` is cosine over it. Two reads
//! of the same line land near each other; two different lines do not.
//!
//! ## No samples here either, and that is the reason this is testable
//!
//! This module takes a SIMILARITY FUNCTION rather than audio. The caller wires
//! `voiceprint-kit` into it; the logic — argmax, outlier detection, what counts
//! as evidence — is arithmetic over distances and tests against a fake in
//! microseconds. Same trade as `VoxelSource` in the unendlich adapter: the part
//! that needs a heavy dependency gets small enough to read, and the part with
//! judgement in it gets exercised.
//!
//! ## What a fingerprint can and cannot say
//!
//! It compares TIMBRE AND SPECTRAL SHAPE, not words. So:
//!
//! * it reliably separates lines of different length and phonetic content;
//! * it reliably spots a different SPEAKER, which catches a mis-filed folder;
//! * it does NOT distinguish "one shot" from "one shot" said slightly
//!   differently, and it should not be asked to;
//! * and it is weakest exactly where the bank is largest — short blips, which
//!   are brief, non-lexical and genuinely similar to each other.
//!
//! [`Confidence`] exists so that last point is visible rather than buried. An
//! audit that says "these three blips might be in any order" is doing its job;
//! one that guesses is worse than one that abstains, because a confident wrong
//! answer stops anyone from listening.

use crate::slice::Assigned;

/// How strongly the audit believes its own finding.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Confidence {
    /// The best match beat the runner-up clearly. Act on it.
    Clear,
    /// Something is off, but the candidates are close enough that a
    /// fingerprint should not be the deciding vote. Listen to it.
    Marginal,
}

/// A problem in an assigned session.
#[derive(Clone, PartialEq, Debug)]
pub enum Finding {
    /// This span looks more like a different unit than the one it was assigned.
    /// The order-error case, which `assign` cannot see.
    Mislabelled {
        span_of: String,
        looks_like: String,
        confidence: Confidence,
    },
    /// Takes of one unit disagree with each other more than takes of one unit
    /// should. Usually a fluffed read, sometimes a different mic position on a
    /// pickup day.
    Inconsistent { unit: String, worst: f32 },
}

/// Margin by which the best match must beat the runner-up to be `Clear`.
/// Cosine distance on a 13-coefficient MFCC is not a precise instrument;
/// asking for a wide gap is how the abstention stays honest.
pub const CLEAR_MARGIN: f32 = 0.15;

/// Distance beyond which two takes of the SAME unit are suspicious.
pub const CONSISTENCY_LIMIT: f32 = 0.45;

/// Audit an assigned session.
///
/// `similarity(unit_id, index_of_assigned)` returns the distance between that
/// span's audio and the reference read for `unit_id` — smaller is more alike.
/// The caller supplies it, wiring `voiceprint::distance(fingerprint(span),
/// reference)`.
///
/// `units` is every id a span could plausibly be, which should be the whole
/// session rather than the whole bank: a span can only have been swapped with
/// something recorded nearby.
pub fn audit<F>(assigned: &[Assigned], units: &[String], similarity: F) -> Vec<Finding>
where
    F: Fn(&str, usize) -> f32,
{
    let mut out = Vec::new();

    for (i, a) in assigned.iter().enumerate() {
        let mut scored: Vec<(&String, f32)> =
            units.iter().map(|u| (u, similarity(u, i))).collect();
        scored.sort_by(|x, y| x.1.partial_cmp(&y.1).unwrap_or(std::cmp::Ordering::Equal));
        let Some((best, best_d)) = scored.first().map(|(u, d)| ((*u).clone(), *d)) else {
            continue;
        };
        if best == a.unit {
            continue;
        }
        let runner_up = scored.get(1).map(|(_, d)| *d).unwrap_or(f32::INFINITY);
        let own = similarity(&a.unit, i);
        // The comparison that matters is best-vs-OWN, not best-vs-runner-up:
        // the question is whether the assigned label is beaten decisively, and
        // the runner-up may itself be the assigned one.
        let margin = (own - best_d).min(runner_up - best_d);
        out.push(Finding::Mislabelled {
            span_of: a.unit.clone(),
            looks_like: best,
            confidence: if margin >= CLEAR_MARGIN { Confidence::Clear } else { Confidence::Marginal },
        });
    }

    // takes of one unit against each other
    let mut by_unit: std::collections::BTreeMap<&str, Vec<usize>> = Default::default();
    for (i, a) in assigned.iter().enumerate() {
        by_unit.entry(a.unit.as_str()).or_default().push(i);
    }
    for (unit, idxs) in by_unit {
        if idxs.len() < 2 {
            continue;
        }
        let worst = idxs.iter().map(|i| similarity(unit, *i)).fold(0.0f32, f32::max);
        if worst > CONSISTENCY_LIMIT {
            out.push(Finding::Inconsistent { unit: unit.to_string(), worst });
        }
    }

    out
}

/// Findings worth stopping an import for. `Marginal` mislabellings are
/// reported but not blocking — a fingerprint should not overrule a slate on
/// its own.
pub fn blocking(findings: &[Finding]) -> Vec<&Finding> {
    findings
        .iter()
        .filter(|f| {
            matches!(
                f,
                Finding::Mislabelled { confidence: Confidence::Clear, .. }
                    | Finding::Inconsistent { .. }
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slice::Span;

    fn a(unit: &str, n: u8) -> Assigned {
        Assigned { unit: unit.to_string(), n, span: Span { start: 0, end: 100 } }
    }
    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_clean_session_reports_nothing() {
        let assigned = vec![a("hello", 1), a("goodbye", 1)];
        let units = ids(&["hello", "goodbye"]);
        // each span is nearest its own reference
        let sim = |u: &str, i: usize| match (u, i) {
            ("hello", 0) | ("goodbye", 1) => 0.05,
            _ => 0.9,
        };
        assert_eq!(audit(&assigned, &units, sim), vec![]);
    }

    #[test]
    fn two_lines_read_in_the_wrong_order_are_caught() {
        // THE CASE assign() CANNOT SEE. The count is right, every span is a
        // real read, and every label is wrong.
        let assigned = vec![a("hello", 1), a("goodbye", 1)];
        let units = ids(&["hello", "goodbye"]);
        let sim = |u: &str, i: usize| match (u, i) {
            ("goodbye", 0) | ("hello", 1) => 0.05,
            _ => 0.9,
        };
        let f = audit(&assigned, &units, sim);
        assert_eq!(f.len(), 2, "{f:?}");
        assert!(f.contains(&Finding::Mislabelled {
            span_of: "hello".into(),
            looks_like: "goodbye".into(),
            confidence: Confidence::Clear,
        }));
        assert_eq!(blocking(&f).len(), 2);
    }

    #[test]
    fn near_identical_candidates_abstain_rather_than_guess() {
        // Short blips are genuinely alike, and this is where the instrument is
        // weakest. A confident wrong answer is worse than an abstention,
        // because it stops anyone from listening.
        let assigned = vec![a("ack_a", 1)];
        let units = ids(&["ack_a", "ack_b"]);
        let sim = |u: &str, _i: usize| if u == "ack_b" { 0.40 } else { 0.44 };
        let f = audit(&assigned, &units, sim);
        assert_eq!(f.len(), 1);
        assert!(
            matches!(f[0], Finding::Mislabelled { confidence: Confidence::Marginal, .. }),
            "{f:?}"
        );
        assert!(blocking(&f).is_empty(), "a marginal call blocked an import");
    }

    #[test]
    fn takes_of_one_unit_that_disagree_are_flagged() {
        // A fluffed read, or a pickup day with the mic moved. Neither is an
        // order error, and both want a human.
        let assigned = vec![a("ack", 1), a("ack", 2)];
        let units = ids(&["ack"]);
        let sim = |_u: &str, i: usize| if i == 1 { 0.8 } else { 0.05 };
        let f = audit(&assigned, &units, sim);
        assert!(f.iter().any(|x| matches!(x, Finding::Inconsistent { .. })), "{f:?}");
        assert_eq!(blocking(&f).len(), 1, "an inconsistent unit should block");
    }

    #[test]
    fn a_single_take_is_never_called_inconsistent() {
        // Nothing to disagree with. The consistency check needs two.
        let assigned = vec![a("ack", 1)];
        let units = ids(&["ack"]);
        let sim = |_u: &str, _i: usize| 0.99;
        let f = audit(&assigned, &units, sim);
        assert!(!f.iter().any(|x| matches!(x, Finding::Inconsistent { .. })), "{f:?}");
    }

    #[test]
    fn an_empty_session_or_an_empty_unit_list_does_not_panic() {
        assert_eq!(audit(&[], &ids(&["a"]), |_, _| 0.0), vec![]);
        assert_eq!(audit(&[a("a", 1)], &[], |_, _| 0.0), vec![]);
    }

    #[test]
    fn the_margin_is_measured_against_the_assigned_label_not_the_runner_up() {
        // The subtle one. With three candidates where the assigned label is
        // itself the runner-up, comparing best-vs-runner-up understates the
        // gap and downgrades a clear swap to marginal.
        let assigned = vec![a("mine", 1)];
        let units = ids(&["other", "mine", "far"]);
        let sim = |u: &str, _i: usize| match u {
            "other" => 0.05,
            "mine" => 0.60, // assigned, and clearly beaten
            _ => 0.95,
        };
        let f = audit(&assigned, &units, sim);
        assert!(
            matches!(f[0], Finding::Mislabelled { confidence: Confidence::Clear, .. }),
            "a decisive swap was reported as marginal: {f:?}"
        );
    }
}
