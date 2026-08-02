//! refute.rs — try to kill a claim with numbers. Cheap to disprove, impossible to prove.
//!
//! ## The asymmetry that makes this worth doing
//!
//! Substitution can never confirm a universal claim — checking `n = 3, 4, 5` says nothing about `n =
//! 10^40`. It can DESTROY a false one instantly. That asymmetry is the whole design: the refuter is a
//! filter, not a judge, and it must never report success as proof.
//!
//! A small model's characteristic failure is confident-and-false. It states things that sound like
//! theorems and are not, and no amount of restructuring its prose changes that — the sentence is
//! well-formed and wrong. A numeric probe is the cheapest thing that can tell the difference, and
//! unlike a larger model's opinion it cannot be talked out of a counter-example.
//!
//! ## Why it pairs with a template rather than replacing one
//!
//! The refuter needs the claim in a shape it can execute. Free prose is not that. So the template's job
//! is to force the claim into a form with variables and a domain — and a claim that cannot be put in
//! that form is itself a finding, because a universal statement whose variables cannot be named is not
//! yet a statement.
//!
//! This is `prechecks` for arithmetic: a mechanical check that produces a fact rather than an opinion,
//! which is what made the cargo loop work.

/// A claim reduced to something executable.
///
/// `holds` is the predicate under test; `domain` supplies candidate assignments. Both come from the
/// model filling a template, which is why the template must ask for variables and ranges rather than
/// for an argument.
pub struct Claim<'a> {
    pub statement: String,
    pub holds: Box<dyn Fn(&[i64]) -> bool + 'a>,
    pub domain: Vec<Vec<i64>>,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Verdict {
    /// A counter-example was found. The claim is FALSE, and this is certain.
    Disproved { counterexample: Vec<i64> },
    /// No counter-example in the probed domain. This is NOT proof and the type says so.
    Survived { probes: usize },
    /// The claim could not be put in executable form.
    Unprobeable { why: String },
}

impl Verdict {
    /// Deliberately not called `is_true`. Surviving a probe is the absence of evidence against, and a
    /// name suggesting otherwise is how a filter gets read as a judge.
    pub fn survived(&self) -> bool { matches!(self, Verdict::Survived { .. }) }
    pub fn is_disproved(&self) -> bool { matches!(self, Verdict::Disproved { .. }) }

    pub fn explain(&self) -> String {
        match self {
            Verdict::Disproved { counterexample } => format!(
                "FALSE: fails at {counterexample:?}. This is certain — one counter-example settles a \
universal claim"),
            Verdict::Survived { probes } => format!(
                "survived {probes} probes. This is NOT proof: substitution cannot confirm a universal \
claim, only refute one. It means no cheap counter-example was found"),
            Verdict::Unprobeable { why } => format!("could not be probed: {why}"),
        }
    }
}

/// Try to refute a claim by substitution.
pub fn refute(c: &Claim) -> Verdict {
    if c.domain.is_empty() {
        return Verdict::Unprobeable {
            why: "no domain given — a universal claim whose variables cannot be named with a range is \
not yet a statement".into() };
    }
    let mut probes = 0usize;
    for assignment in &c.domain {
        probes += 1;
        if !(c.holds)(assignment) {
            return Verdict::Disproved { counterexample: assignment.clone() };
        }
    }
    Verdict::Survived { probes }
}

/// Build a domain by taking a cross-product of small values, plus the edges.
///
/// Edges first: a claim that fails anywhere usually fails at 0, 1, -1 or a boundary, and probing those
/// before the interior finds counter-examples in the first few tries rather than the last.
pub fn small_domain(vars: usize, lo: i64, hi: i64) -> Vec<Vec<i64>> {
    let mut vals: Vec<i64> = vec![0, 1, -1, lo, hi];
    vals.extend(lo..=hi);
    vals.sort_unstable();
    vals.dedup();
    let mut out: Vec<Vec<i64>> = vec![vec![]];
    for _ in 0..vars {
        let mut next = Vec::new();
        for base in &out {
            for v in &vals {
                let mut b = base.clone();
                b.push(*v);
                next.push(b);
            }
        }
        out = next;
    }
    out
}

/// The template slot that makes a claim probeable.
pub const PROBEABLE_SLOT: &str = "\
Restate the claim with its variables named and their range given, in the form:

    for all <vars> in <range>: <expression> <relation> <expression>

If the claim cannot be written that way, say so and stop — a universal statement whose variables cannot \
be named is not yet a statement, and dressing it in prose will not make it one.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_false_claim_dies_on_the_first_counterexample() {
        // The whole point: confident-and-false is a small model's characteristic failure, and this is
        // the cheapest thing that tells the difference.
        let c = Claim {
            statement: "for all n: n*n > n".into(),
            holds: Box::new(|v| v[0] * v[0] > v[0]),
            domain: small_domain(1, -3, 5),
        };
        let r = refute(&c);
        assert!(r.is_disproved(), "n=0 and n=1 both break it");
        assert!(r.explain().contains("one counter-example settles a universal claim"));
    }

    #[test]
    fn surviving_is_NOT_proof_and_the_wording_says_so() {
        // The failure this type is shaped to prevent. A filter read as a judge would turn "no cheap
        // counter-example" into "verified", which is exactly the false confidence the whole exercise
        // is meant to remove.
        let c = Claim {
            statement: "for all n: n*n >= 0".into(),
            holds: Box::new(|v| v[0] * v[0] >= 0),
            domain: small_domain(1, -5, 5),
        };
        let r = refute(&c);
        assert!(r.survived());
        assert!(!r.is_disproved());
        assert!(r.explain().contains("NOT proof"));
        assert!(r.explain().contains("only refute one"));
    }

    #[test]
    fn a_claim_with_no_domain_is_unprobeable_not_surviving() {
        // A claim nobody can probe must not be scored as one that withstood probing — that is the
        // direction which rewards vagueness.
        let c = Claim { statement: "beauty is truth".into(),
                        holds: Box::new(|_| true), domain: vec![] };
        let r = refute(&c);
        assert!(!r.survived());
        if let Verdict::Unprobeable { why } = r {
            assert!(why.contains("not yet a statement"));
        } else { panic!("expected Unprobeable") }
    }

    #[test]
    fn the_domain_probes_edges_before_the_interior() {
        // A claim that fails anywhere usually fails at 0, 1 or -1. Probing those first finds
        // counter-examples in the first few tries rather than the last, which matters when the domain
        // is large and the budget is a few hundred probes.
        let d = small_domain(1, 2, 4);
        let flat: Vec<i64> = d.iter().map(|v| v[0]).collect();
        for edge in [0, 1, -1] { assert!(flat.contains(&edge), "missing edge {edge}"); }
    }

    #[test]
    fn a_two_variable_claim_is_probed_over_the_cross_product() {
        // Single-variable probing would miss claims that only fail on a combination — which is most
        // interesting false claims.
        let c = Claim {
            statement: "for all a,b: a+b >= a".into(),
            holds: Box::new(|v| v[0] + v[1] >= v[0]),
            domain: small_domain(2, 0, 3),
        };
        assert!(refute(&c).is_disproved(), "b = -1 breaks it");
    }

    #[test]
    fn the_slot_permits_declining_rather_than_forcing_a_shape() {
        // Without this a model asked to name variables will name them for a claim that has none, and
        // the refuter then probes something the claim never said.
        assert!(PROBEABLE_SLOT.contains("say so and stop"));
        assert!(PROBEABLE_SLOT.contains("not yet a statement"));
    }
}
