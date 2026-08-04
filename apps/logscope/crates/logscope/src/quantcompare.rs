//! quantcompare.rs — does the smaller quant actually write worse code?
//!
//! The question cannot be answered by running both and reading the output. Two generations differ
//! because of sampling before they differ because of quantization, so eyeballing two turtle games
//! compares noise.
//!
//! ## Why perplexity is the wrong measure here
//!
//! Perplexity is the standard answer and it measures the wrong thing for this use. A model can lose
//! two points of perplexity and still write compiling Rust, or gain a point and start emitting
//! `_ else if`. **The question is not "how surprised is it" but "does the code build".**
//!
//! ## The measure this project already has
//!
//! `patchcheck` tells whether generated code is structurally sound, and `buildsignal` tells whether
//! a build succeeded. Both are BINARY and verifiable — no judgement, no rubric, no second model
//! grading the first. That is a far better foundation than a quality score, and it exists.
//!
//! ## Controlling the comparison
//!
//! Same prompt, same seed, same sampler. **Greedy decoding where possible**, because temperature
//! turns a comparison of two models into a comparison of two dice rolls, and the whole point is to
//! attribute the difference to the quant.

/// One generation, scored on things that are checkable.
#[derive(Clone, PartialEq, Debug)]
pub struct Sample {
    /// The quant this came from, e.g. "Q4_K_M" or "IQ2_XXS".
    pub quant: String,
    /// Did the emitted code parse — balanced delimiters, no unterminated strings?
    pub structurally_valid: bool,
    /// Did it compile, if it was run through a build?
    pub compiled: Option<bool>,
    /// Tokens produced. A model that degrades often gets SHORTER as well as worse, and a length
    /// collapse is a signal in its own right.
    pub tokens: u32,
    /// Wall seconds, so the tradeoff has both axes.
    pub secs: f32,
}

impl Sample {
    pub fn tok_s(&self) -> f32 { if self.secs > 0.0 { self.tokens as f32 / self.secs } else { 0.0 } }
}

/// The verdict on a pair of quants over the same prompts.
#[derive(Clone, PartialEq, Debug)]
pub enum Verdict {
    /// Not enough samples for the difference to mean anything.
    Inconclusive { reason: String },
    /// The smaller quant held up.
    NoRegression { speedup: f32 },
    /// It got worse, and by how much.
    Degraded { valid_drop: f32, speedup: f32 },
}

impl Verdict {
    pub fn explain(&self) -> String {
        match self {
            Verdict::Inconclusive { reason } => format!(
                "inconclusive: {reason}. Reporting a difference from this many samples would be \
reading sampling noise as a quantization effect"),
            Verdict::NoRegression { speedup } => format!(
                "no measurable regression, at {speedup:.2}x the throughput. The smaller quant is \
free on this workload — which is a claim about THIS workload and not about the model"),
            Verdict::Degraded { valid_drop, speedup } => format!(
                "{:.0}% fewer valid outputs, for {speedup:.2}x the throughput. Whether that trade is \
worth taking depends on what the outputs are for; the measurement does not decide it",
                valid_drop * 100.0),
        }
    }
}

/// Minimum samples per quant before a difference is reported.
///
/// **Ten, and it is a floor rather than a target.** One prompt tells you nothing: a single failure
/// can be the prompt rather than the quant, which is exactly how a bad conclusion gets drawn from a
/// convincing anecdote.
pub const MIN_SAMPLES: usize = 10;

/// Compare two sets of samples on the same prompts.
pub fn compare(baseline: &[Sample], candidate: &[Sample]) -> Verdict {
    if baseline.len() < MIN_SAMPLES || candidate.len() < MIN_SAMPLES {
        return Verdict::Inconclusive { reason: format!(
            "{} and {} samples against a floor of {MIN_SAMPLES}", baseline.len(), candidate.len()) };
    }
    if baseline.len() != candidate.len() {
        return Verdict::Inconclusive { reason:
            "different sample counts — the same prompts must be run through both, or the comparison \
is between two different workloads".into() };
    }
    let rate = |s: &[Sample]| {
        // Compilation is the stronger signal and structural validity the weaker one; use the
        // stronger where it is available, since a file can be balanced and still not build.
        s.iter().filter(|x| x.compiled.unwrap_or(x.structurally_valid)).count() as f32
            / s.len() as f32
    };
    let (b, c) = (rate(baseline), rate(candidate));
    let tps = |s: &[Sample]| s.iter().map(|x| x.tok_s()).sum::<f32>() / s.len() as f32;
    let speedup = if tps(baseline) > 0.0 { tps(candidate) / tps(baseline) } else { 1.0 };
    // A single sample either way is within what ten samples can resolve, so a drop must exceed one
    // sample's worth to be called a regression at all.
    let resolution = 1.0 / baseline.len() as f32;
    // Epsilon, because the boundary case IS the common one: ten samples with one failure gives
    // 1.0 - 0.9 = 0.100000024 in f32, which is greater than 0.1 and would be called a regression on
    // a rounding artefact. This project has spent a session on exactly that class of bug, and it is
    // the last place it should appear.
    if b - c > resolution * 1.001 { Verdict::Degraded { valid_drop: b - c, speedup } }
    else { Verdict::NoRegression { speedup } }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(q: &str, ok: bool, tps: f32) -> Sample {
        Sample { quant: q.into(), structurally_valid: ok, compiled: Some(ok),
                 tokens: (tps * 10.0) as u32, secs: 10.0 }
    }

    #[test]
    fn ONE_PROMPT_is_refused_rather_than_answered() {
        // A single failure can be the prompt rather than the quant, which is how a bad conclusion
        // gets drawn from a convincing anecdote.
        let v = compare(&[s("q4", true, 5.0)], &[s("iq2", false, 20.0)]);
        assert!(matches!(v, Verdict::Inconclusive { .. }));
        assert!(v.explain().contains("reading sampling noise as a quantization effect"));
    }

    #[test]
    fn a_REAL_regression_is_reported_with_BOTH_axes() {
        // The trade has two sides and reporting one is how a decision gets made badly.
        let base: Vec<_> = (0..10).map(|_| s("q4", true, 5.0)).collect();
        let cand: Vec<_> = (0..10).map(|i| s("iq2", i < 6, 20.0)).collect();
        let v = compare(&base, &cand);
        match v {
            Verdict::Degraded { valid_drop, speedup } => {
                assert!((valid_drop - 0.4).abs() < 0.01);
                assert!((speedup - 4.0).abs() < 0.01);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_SINGLE_sample_difference_is_within_the_resolution() {
        // Ten samples cannot resolve one sample. Calling that a regression would manufacture a
        // finding out of the sample count.
        let base: Vec<_> = (0..10).map(|_| s("q4", true, 5.0)).collect();
        let cand: Vec<_> = (0..10).map(|i| s("iq2", i < 9, 10.0)).collect();
        assert!(matches!(compare(&base, &cand), Verdict::NoRegression { .. }));
    }

    #[test]
    fn MISMATCHED_prompt_sets_are_refused() {
        // Otherwise the comparison is between two different workloads wearing one label.
        let base: Vec<_> = (0..12).map(|_| s("q4", true, 5.0)).collect();
        let cand: Vec<_> = (0..10).map(|_| s("iq2", true, 5.0)).collect();
        assert!(matches!(compare(&base, &cand), Verdict::Inconclusive { .. }));
    }

    #[test]
    fn COMPILATION_outranks_structural_validity_when_available() {
        // A file can be balanced and still not build, so the stronger signal wins where it exists.
        let base: Vec<_> = (0..10).map(|_| s("q4", true, 5.0)).collect();
        let cand: Vec<_> = (0..10).map(|_| Sample {
            quant: "iq2".into(), structurally_valid: true, compiled: Some(false),
            tokens: 50, secs: 10.0 }).collect();
        assert!(matches!(compare(&base, &cand), Verdict::Degraded { .. }));
    }

    #[test]
    fn NO_REGRESSION_is_stated_as_a_claim_about_the_WORKLOAD() {
        // Not about the model. Ten coding prompts say nothing about summarisation.
        let base: Vec<_> = (0..10).map(|_| s("q4", true, 5.0)).collect();
        let cand: Vec<_> = (0..10).map(|_| s("iq2", true, 15.0)).collect();
        assert!(compare(&base, &cand).explain().contains("about THIS workload and not about the model"));
    }

    #[test]
    fn the_verdict_does_NOT_decide_whether_the_trade_is_worth_it() {
        // "40% fewer valid outputs for 4x throughput" is a measurement. Whether to take it depends on
        // what the outputs are for, and the harness should not pretend to know.
        let base: Vec<_> = (0..10).map(|_| s("q4", true, 5.0)).collect();
        let cand: Vec<_> = (0..10).map(|i| s("iq2", i < 6, 20.0)).collect();
        assert!(compare(&base, &cand).explain().contains("the measurement does not decide it"));
    }
}
