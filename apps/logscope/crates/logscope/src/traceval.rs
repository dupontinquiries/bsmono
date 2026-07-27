//! traceval.rs — score code by what it DID, not by what a model thinks of it.
//!
//! Borrowed from how a JIT decides what to optimize: it does not reason about the source, it counts.
//! Which paths ran, how often, which assumptions held, which blew up. Applied to validating generated
//! code, the same counters answer questions a model can only guess at.
//!
//! ## Why this beats a model reading the code
//!
//! A model reading a function predicts what it would do. Instrumented execution observes what it did.
//! For the questions that matter — does this branch ever run, does this loop terminate, does this
//! unwrap hold — observation is both cheaper and categorically better evidence. A counter increment is
//! nanoseconds; a Thinker's opinion is ~30 Disciple forward passes and can be confidently wrong.
//!
//! ## The one thing this must never be read as
//!
//! **Coverage is evidence of EXECUTION, not of CORRECTNESS.** Code that ran without panicking may still
//! compute the wrong answer. This module therefore scores *confidence in the evidence*, never
//! correctness itself, and a span with no executions is reported as UNVALIDATED rather than as passing.
//! Treating untouched code as fine is the single most common misreading of a coverage report, and it is
//! exactly the misreading that lets a generated function with a broken error path ship.
//!
//! ## The tiering, which is the JIT part
//!
//! A JIT does not optimize cold code, because the work would not pay for itself. The same applies here:
//! a Thinker's review should go to code that RUNS, and hot code that has never failed is far better
//! attested than cold code nobody has exercised. `review_order` sorts on exactly that.

use std::collections::BTreeMap;

/// What happened at one instrumented span (a function, a branch, a block).
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct SpanStats {
    /// Times the span was entered.
    pub hits: u64,
    /// Times it raised — panic, error return, failed assertion.
    pub faults: u64,
    /// Distinct shapes/types seen, where the harness can observe them. 1 is monomorphic and stable;
    /// many suggests the span is doing more than one job, which a JIT would treat as a deopt risk and a
    /// reviewer should treat as a smell.
    pub shapes: u32,
}

impl SpanStats {
    pub fn never_ran(&self) -> bool { self.hits == 0 }
    pub fn fault_rate(&self) -> f32 {
        if self.hits == 0 { 0.0 } else { self.faults as f32 / self.hits as f32 }
    }
}

/// How well-attested a span is by execution evidence.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Attestation {
    /// Never executed. NOT a pass — nothing is known about it.
    Unvalidated,
    /// Ran, and faulted. Decisive negative evidence, unlike any model opinion.
    Faulted,
    /// Ran a little, no faults. Weak positive.
    Exercised,
    /// Ran a lot, no faults, stable shapes. The strongest evidence available here.
    WellAttested,
}

impl Attestation {
    /// Does a human or Thinker still need to look?
    ///
    /// `Unvalidated` counts, and that is the point: untouched code has had no review of any kind, which
    /// is a different risk from reviewed-and-fine but the same requirement — someone must look.
    pub fn needs_review(self) -> bool { !matches!(self, Attestation::WellAttested) }

    pub fn explain(self) -> &'static str {
        match self {
            Attestation::Unvalidated => "never executed — nothing is known about it, which is not the \
same as nothing being wrong with it",
            Attestation::Faulted => "raised during execution — direct evidence, not an opinion",
            Attestation::Exercised => "ran without faulting, but not enough times to lean on",
            Attestation::WellAttested => "ran often, never faulted, stable shapes",
        }
    }
}

/// Threshold above which a span counts as hot. Mirrors a JIT's tier-up counter: low enough that a
/// realistic test exercise clears it, high enough that a single incidental call does not.
pub const HOT: u64 = 50;

pub fn attest(s: &SpanStats) -> Attestation {
    if s.hits == 0 { return Attestation::Unvalidated; }
    if s.faults > 0 { return Attestation::Faulted; }
    if s.hits >= HOT && s.shapes <= 1 { Attestation::WellAttested } else { Attestation::Exercised }
}

/// Execution evidence for one candidate implementation.
#[derive(Clone, Debug, Default)]
pub struct Trace { spans: BTreeMap<String, SpanStats> }

impl Trace {
    pub fn new() -> Self { Self::default() }

    /// Record `n` executions of a span, of which `faults` raised.
    pub fn record(&mut self, span: &str, n: u64, faults: u64, shapes: u32) {
        let e = self.spans.entry(span.to_string()).or_default();
        e.hits += n;
        e.faults += faults;
        e.shapes = e.shapes.max(shapes);
    }

    /// Declare a span exists without executing it — so it can be reported as Unvalidated rather than
    /// being invisible.
    ///
    /// Without this, a function nobody called simply does not appear, and an absent span reads as "no
    /// problems here". Untouched code has to be VISIBLE to be reviewed.
    pub fn declare(&mut self, span: &str) { self.spans.entry(span.to_string()).or_default(); }

    pub fn get(&self, span: &str) -> Option<&SpanStats> { self.spans.get(span) }
    pub fn is_empty(&self) -> bool { self.spans.is_empty() }

    pub fn attestation(&self, span: &str) -> Option<Attestation> { self.spans.get(span).map(attest) }

    /// Spans needing review, worst first: faults, then never-ran, then thinly-exercised.
    ///
    /// Faults outrank never-ran because a fault is a KNOWN defect while an unrun span is an unknown, and
    /// a known defect is always the better use of the next minute.
    pub fn review_order(&self) -> Vec<(&str, Attestation)> {
        let mut v: Vec<(&str, Attestation)> = self.spans.iter()
            .map(|(k, s)| (k.as_str(), attest(s)))
            .filter(|(_, a)| a.needs_review()).collect();
        let rank = |a: Attestation| match a {
            Attestation::Faulted => 0, Attestation::Unvalidated => 1,
            Attestation::Exercised => 2, Attestation::WellAttested => 3 };
        v.sort_by_key(|(name, a)| (rank(*a), *name));
        v
    }

    /// Fraction of declared spans that are WellAttested. None when nothing was declared.
    pub fn attested_fraction(&self) -> Option<f32> {
        if self.spans.is_empty() { return None; }
        let n = self.spans.values().filter(|s| attest(s) == Attestation::WellAttested).count();
        Some(n as f32 / self.spans.len() as f32)
    }

    /// Operator summary.
    pub fn summary(&self) -> Vec<String> {
        let mut out = Vec::new();
        let Some(frac) = self.attested_fraction() else { return vec!["no spans instrumented".into()] };
        out.push(format!("{:.0}% of {} spans well-attested by execution", frac * 100.0, self.spans.len()));
        for (name, a) in self.review_order().iter().take(6) {
            out.push(format!("  {name}: {}", a.explain()));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_that_never_ran_is_UNVALIDATED_not_passing() {
        // The single most common misreading of a coverage report, and the one that lets a generated
        // function with a broken error path ship: an untouched span looks clean because nothing went
        // wrong in it.
        let mut t = Trace::new();
        t.declare("handle_error");
        assert_eq!(t.attestation("handle_error"), Some(Attestation::Unvalidated));
        assert!(Attestation::Unvalidated.needs_review());
        assert!(Attestation::Unvalidated.explain().contains("not the same as nothing being wrong"));
    }

    #[test]
    fn a_declared_but_unrun_span_is_VISIBLE() {
        // Without declare(), a function nobody called does not appear at all — and an absent span reads
        // as "no problems here". It has to be visible to be reviewed.
        let mut t = Trace::new();
        t.declare("cold_path");
        t.record("hot_path", 100, 0, 1);
        let q = t.review_order();
        assert!(q.iter().any(|(n, _)| *n == "cold_path"), "{q:?}");
        assert!(!q.iter().any(|(n, _)| *n == "hot_path"), "well-attested spans leave the queue");
    }

    #[test]
    fn a_fault_outranks_an_unrun_span_in_the_queue() {
        // A fault is a KNOWN defect; an unrun span is an unknown. The known defect is always the better
        // use of the next minute.
        let mut t = Trace::new();
        t.declare("never");
        t.record("broken", 10, 3, 1);
        let q = t.review_order();
        assert_eq!(q[0].0, "broken");
        assert_eq!(q[0].1, Attestation::Faulted);
        assert_eq!(q[1].0, "never");
    }

    #[test]
    fn hot_and_clean_beats_cold_and_clean() {
        // The JIT part: a tier-up counter. Ten clean calls is not the same evidence as a thousand, and
        // treating them alike wastes a Thinker on code that is already well attested.
        let mut t = Trace::new();
        t.record("cold", HOT - 1, 0, 1);
        t.record("hot", HOT, 0, 1);
        assert_eq!(t.attestation("cold"), Some(Attestation::Exercised));
        assert_eq!(t.attestation("hot"), Some(Attestation::WellAttested));
        assert!(Attestation::Exercised.needs_review());
        assert!(!Attestation::WellAttested.needs_review());
    }

    #[test]
    fn shape_instability_keeps_a_hot_span_in_the_queue() {
        // A JIT deoptimizes a polymorphic site; a reviewer should look at one. A span doing more than
        // one job is a smell even when it never faults.
        let mut t = Trace::new();
        t.record("poly", 1000, 0, 4);
        assert_eq!(t.attestation("poly"), Some(Attestation::Exercised),
            "hot and clean, but not monomorphic");
        assert!(t.review_order().iter().any(|(n, _)| *n == "poly"));
    }

    #[test]
    fn faults_accumulate_across_records_rather_than_overwriting() {
        let mut t = Trace::new();
        t.record("f", 5, 1, 1);
        t.record("f", 5, 2, 1);
        let s = t.get("f").unwrap();
        assert_eq!((s.hits, s.faults), (10, 3));
        assert!((s.fault_rate() - 0.3).abs() < 1e-5);
    }

    #[test]
    fn nothing_instrumented_reports_nothing_rather_than_zero_percent() {
        // "0% attested" and "we did not measure" are different claims, and only one of them should make
        // someone go looking.
        let t = Trace::new();
        assert!(t.attested_fraction().is_none());
        assert_eq!(t.summary(), vec!["no spans instrumented".to_string()]);
        assert!(t.is_empty());
    }

    #[test]
    fn a_fault_rate_on_zero_hits_is_zero_not_a_division_by_zero() {
        let s = SpanStats::default();
        assert!(s.never_ran());
        assert_eq!(s.fault_rate(), 0.0);
    }
}
