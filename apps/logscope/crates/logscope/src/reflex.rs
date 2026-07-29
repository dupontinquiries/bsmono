//! reflex.rs — cheap judgements read from logits, never sampled.
//!
//! Two questions a small model can answer in roughly one forward pass:
//!
//! 1. **"Is this code good?"** — ask, then read `P(yes)` against `P(no)` in the next-token distribution.
//! 2. **"Would you have written this?"** — teacher-force the existing tokens and read their logprobs.
//!    No prompt at all; the text IS the question.
//!
//! ## Why read rather than sample
//!
//! Sampling a verdict token throws away almost everything. The model computed a distribution over the
//! whole vocabulary; taking one sample from it yields a word, when the distribution itself is a
//! calibrated number. Reading it directly is cheaper (no sampling, no parsing), and it CANNOT return a
//! third answer — a sampled verdict can reply "maybe", "Yes.", or a newline, and then someone writes a
//! parser, and the parser becomes the bug.
//!
//! It also preserves the distinction that matters most: a confident NO and a genuine DON'T KNOW are
//! different signals with different responses, and argmax collapses them into the same token.
//!
//! ## What these are not
//!
//! Advisory. Nothing here gates correctness — a low score routes work to a stronger model or flags a
//! span for review; it never rewrites, skips or approves anything on its own. The moment a cheap
//! predictor is allowed to substitute for the real computation, its errors become the output's errors,
//! which is the failure mode this codebase has spent its whole history hunting.

/// A verdict read from a next-token distribution.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Verdict {
    /// P(yes) / (P(yes) + P(no)) — the opinion, ignoring everything else.
    pub score: f32,
    /// How much probability mass landed on the answer set at all. Low means the model did not treat
    /// this as a yes/no question, and the score above is then meaningless however extreme it looks.
    pub attention: f32,
}

impl Verdict {
    /// Is the verdict usable? A confident-looking score built from 2% of the mass is noise.
    pub fn is_meaningful(&self) -> bool { self.attention >= 0.10 }

    /// Advisory action. Deliberately three-valued: collapsing "unsure" into "no" wastes the one thing a
    /// probabilistic judge is better at than a binary one.
    pub fn call(&self) -> &'static str {
        if !self.is_meaningful() { return "no opinion — the model did not answer the question asked" }
        if self.score >= 0.80 { "pass" } else if self.score <= 0.20 { "fail" } else { "unsure — escalate" }
    }
}

/// Read a yes/no verdict from a next-token probability distribution.
///
/// `probs` is the full softmaxed distribution; the two id lists are the token ids that spell yes and no
/// (several, because " yes", "Yes", "YES" and "yes" are distinct tokens and a model may prefer any).
pub fn verdict_from_probs(probs: &[f32], yes_ids: &[usize], no_ids: &[usize]) -> Verdict {
    let sum = |ids: &[usize]| ids.iter().filter_map(|&i| probs.get(i)).sum::<f32>();
    let (y, n) = (sum(yes_ids), sum(no_ids));
    let total = y + n;
    if total <= 0.0 { return Verdict { score: 0.5, attention: 0.0 }; }
    Verdict { score: y / total, attention: total.min(1.0) }
}

/// How well a model agrees with text it did not write.
///
/// Mean logprob per token under teacher forcing — "would I have written this?" Length-normalized,
/// because a long correct function must not score worse than a short one simply for being long.
#[derive(Clone, Debug, PartialEq)]
pub struct Agreement {
    /// Mean logprob per token. Closer to 0 is stronger agreement.
    pub mean_logprob: f32,
    /// Token positions whose logprob is far below the mean — where the model would have written
    /// something else. This is the useful output: a scalar says "somewhat odd", these say WHERE.
    pub surprises: Vec<usize>,
}

impl Agreement {
    /// Perplexity, for readers who think in those units.
    pub fn perplexity(&self) -> f32 { (-self.mean_logprob).exp() }
}

/// Score existing tokens under a model's own predictions.
///
/// `token_logprobs[i]` is the logprob the model assigned to the token that actually appeared at `i`.
/// `sigma` is how many standard deviations below the mean counts as a surprise.
pub fn agreement(token_logprobs: &[f32], sigma: f32) -> Option<Agreement> {
    if token_logprobs.is_empty() { return None; }
    let n = token_logprobs.len() as f32;
    let mean = token_logprobs.iter().sum::<f32>() / n;
    let var = token_logprobs.iter().map(|l| (l - mean).powi(2)).sum::<f32>() / n;
    let sd = var.sqrt();
    // A uniform sequence has sd 0 and therefore no surprises — correct: nothing stands out.
    let cut = mean - sigma * sd;
    let surprises = token_logprobs.iter().enumerate()
        .filter(|(_, &l)| sd > 0.0 && l < cut).map(|(i, _)| i).collect();
    Some(Agreement { mean_logprob: mean, surprises })
}

/// The prompt for a verdict. Short on purpose: it is read once per candidate and the answer is a single
/// distribution, so every extra token is pure cost.
pub fn verdict_prompt(code: &str, question: &str) -> String {
    // FRAMED as candidate output: an error in this block means the CANDIDATE is wrong, not that the tool
    // broke or that the user's program failed. A judge that mistakes the frame answers about the wrong
    // artifact — fluently, because nothing in a stack trace says which of the three it came from.
    format!("{}\n{code}\n\n{question}\nAnswer with one word, yes or no:",
        crate::promptframe::Frame::CandidateOutput.banner())
}

/// A rung on the escalation ladder: who is asked, and what it costs.
///
/// Ordered cheapest-first, and each rung only runs if the one below it was INCONCLUSIVE. A confident
/// answer from a cheap judge is not improved by asking an expensive one — the point of the ladder is to
/// spend a Thinker's forward pass only on the cases that are actually ambiguous.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub enum Rung {
    /// Deterministic checks. Microseconds, no model.
    Prechecks,
    /// Execution evidence: counters from actually running the code. Nanoseconds per span, and
    /// categorically better than an opinion for the questions it can answer — a span that FAULTED is a
    /// known defect, not a guess.
    Trace,
    /// The Disciple reads a verdict from one distribution.
    DiscipleVerdict,
    /// A second Disciple, differently prompted or a different model. Disagreement between two cheap
    /// judges is itself the signal to escalate.
    SecondOpinion,
    /// The Thinker. One real forward pass over the code.
    Thinker,
}

impl Rung {
    /// Rough cost in "Disciple forward passes", for reasoning about the ladder's economics.
    pub fn cost(self) -> f32 {
        match self { Rung::Prechecks => 0.0, Rung::Trace => 0.0, Rung::DiscipleVerdict => 1.0,
                     Rung::SecondOpinion => 2.0, Rung::Thinker => 30.0 }
    }
}

/// What the ladder decided, and how far it had to climb.
#[derive(Clone, PartialEq, Debug)]
pub struct Ruling { pub rung: Rung, pub pass: Option<bool>, pub why: String }

/// Walk the ladder. `precheck_blocking` is the count of Certain findings; the closures are only CALLED
/// when their rung is reached, so an unused rung costs nothing.
///
/// Two rules the tests pin down:
/// - A blocking precheck ends it immediately. Asking a model whether code compiles, when a regex already
///   knows it does not, spends a forward pass to be told what was free.
/// - Two cheap judges DISAGREEING escalates, even though each was individually confident. That
///   disagreement is exactly the ambiguity the Thinker exists for, and averaging them would hide it.
/// As `walk`, with execution evidence consulted before any model.
///
/// A fault is decisive: the code demonstrably raised, and no model opinion improves on that. But a clean
/// trace is NOT a pass — coverage proves execution, not correctness — so it only *lowers* the priority
/// and the ladder continues. That asymmetry is the whole point: evidence of failure is conclusive,
/// evidence of success is not.
pub fn walk_traced(
    trace_fault: bool,
    precheck_blocking: usize,
    first: impl FnOnce() -> Verdict,
    second: impl FnOnce() -> Verdict,
    thinker: impl FnOnce() -> bool,
) -> Ruling {
    if precheck_blocking > 0 {
        return Ruling { rung: Rung::Prechecks, pass: Some(false),
            why: format!("{precheck_blocking} deterministic finding(s) — no model needed") };
    }
    if trace_fault {
        return Ruling { rung: Rung::Trace, pass: Some(false),
            why: "the code raised when run — direct evidence, and no model opinion improves on it".into() };
    }
    walk(0, first, second, thinker)
}

pub fn walk(
    precheck_blocking: usize,
    first: impl FnOnce() -> Verdict,
    second: impl FnOnce() -> Verdict,
    thinker: impl FnOnce() -> bool,
) -> Ruling {
    if precheck_blocking > 0 {
        return Ruling { rung: Rung::Prechecks, pass: Some(false),
            why: format!("{precheck_blocking} deterministic finding(s) — no model needed") };
    }
    let a = first();
    if a.is_meaningful() && (a.score >= 0.80 || a.score <= 0.20) {
        // A confident cheap answer is not improved by an expensive one.
        return Ruling { rung: Rung::DiscipleVerdict, pass: Some(a.score >= 0.80),
            why: format!("disciple confident ({:.2})", a.score) };
    }
    let b = second();
    let both_meaningful = a.is_meaningful() && b.is_meaningful();
    let agree = (a.score >= 0.5) == (b.score >= 0.5);
    if both_meaningful && agree && (b.score >= 0.80 || b.score <= 0.20) {
        return Ruling { rung: Rung::SecondOpinion, pass: Some(b.score >= 0.80),
            why: format!("two disciples agree ({:.2}, {:.2})", a.score, b.score) };
    }
    let why = if both_meaningful && !agree {
        format!("disciples DISAGREE ({:.2} vs {:.2}) — that is the ambiguity a thinker is for",
            a.score, b.score)
    } else {
        "no confident cheap verdict".to_string()
    };
    Ruling { rung: Rung::Thinker, pass: Some(thinker()), why }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dist(pairs: &[(usize, f32)], n: usize) -> Vec<f32> {
        let mut v = vec![0.0; n];
        for &(i, p) in pairs { v[i] = p; }
        v
    }

    #[test]
    fn a_verdict_is_read_not_sampled_and_cannot_return_a_third_answer() {
        // Sampling yields a word — "maybe", "Yes.", a newline — and then someone writes a parser, and
        // the parser becomes the bug. Reading gives a number over a fixed answer set.
        let p = dist(&[(10, 0.7), (20, 0.1), (99, 0.2)], 100);
        let v = verdict_from_probs(&p, &[10], &[20]);
        assert!((v.score - 0.875).abs() < 1e-3, "0.7 / (0.7+0.1)");
        assert!(v.is_meaningful());
        assert_eq!(v.call(), "pass");
    }

    #[test]
    fn several_spellings_of_yes_all_count() {
        // " yes", "Yes", "YES" are distinct tokens and a model may prefer any of them; counting only one
        // would read a confident yes as no opinion.
        let p = dist(&[(1, 0.3), (2, 0.3), (3, 0.05)], 10);
        let v = verdict_from_probs(&p, &[1, 2], &[3]);
        assert!(v.score > 0.9);
        assert!((v.attention - 0.65).abs() < 1e-3);
    }

    #[test]
    fn a_confident_score_built_on_no_mass_is_reported_as_no_opinion() {
        // THE failure mode of this technique. If the model spent 98% of its mass elsewhere, it did not
        // answer the question — and 1.0 vs 0.0 within the remaining 2% looks maximally confident.
        let p = dist(&[(1, 0.02), (2, 0.0), (7, 0.98)], 10);
        let v = verdict_from_probs(&p, &[1], &[2]);
        assert_eq!(v.score, 1.0, "unanimous within the answer set");
        assert!(!v.is_meaningful(), "but built on 2% of the mass");
        assert!(v.call().contains("did not answer the question"));
    }

    #[test]
    fn unsure_is_its_own_answer_not_a_failure() {
        // Collapsing "unsure" into "no" wastes the one thing a probabilistic judge does better than a
        // binary one.
        let p = dist(&[(1, 0.3), (2, 0.3)], 10);
        assert_eq!(verdict_from_probs(&p, &[1], &[2]).call(), "unsure — escalate");
        let empty = dist(&[], 10);
        let v = verdict_from_probs(&empty, &[1], &[2]);
        assert_eq!(v.attention, 0.0);
        assert!(!v.is_meaningful(), "no mass at all is no opinion, not a tie");
    }

    #[test]
    fn agreement_is_length_normalized() {
        // A long correct function must not score worse than a short one just for being long.
        let short = agreement(&[-0.5, -0.5], 2.0).unwrap();
        let long = agreement(&[-0.5; 40], 2.0).unwrap();
        assert!((short.mean_logprob - long.mean_logprob).abs() < 1e-6);
    }

    #[test]
    fn agreement_localizes_WHERE_the_model_disagrees() {
        // The scalar says "somewhat odd"; the positions say which line to look at. That is the whole
        // value — a perplexity number alone is not actionable.
        let mut lp = vec![-0.2f32; 20];
        lp[7] = -9.0;                      // one wildly unexpected token
        let a = agreement(&lp, 2.0).unwrap();
        assert_eq!(a.surprises, vec![7]);
        assert!(a.perplexity() > 1.0);
    }

    #[test]
    fn a_uniform_sequence_has_no_surprises() {
        // sd = 0: nothing stands out, and a detector that fired here would flag every well-predicted
        // passage as suspicious.
        let a = agreement(&[-1.0; 12], 2.0).unwrap();
        assert!(a.surprises.is_empty());
        assert!((a.perplexity() - 1.0f32.exp()).abs() < 1e-4);
    }

    fn v(score: f32, attention: f32) -> Verdict { Verdict { score, attention } }

    #[test]
    fn a_blocking_precheck_never_wakes_a_model() {
        // Asking a model whether code compiles, when a regex already knows it does not, spends a forward
        // pass to be told what was free.
        let r = walk(3, || panic!("must not run"), || panic!("must not run"), || panic!("must not run"));
        assert_eq!(r.rung, Rung::Prechecks);
        assert_eq!(r.pass, Some(false));
        assert!(r.why.contains("no model needed"));
    }

    #[test]
    fn a_fault_in_the_trace_settles_it_without_any_model() {
        // Running the code is nanoseconds and a fault is a KNOWN defect. Asking a model whether code is
        // good, when it has already been observed to raise, spends 30x the cost to be told what running
        // it proved.
        let r = walk_traced(true, 0, || panic!("no model"), || panic!("no model"), || panic!("no model"));
        assert_eq!(r.rung, Rung::Trace);
        assert_eq!(r.pass, Some(false));
        assert_eq!(Rung::Trace.cost(), 0.0);
    }

    #[test]
    fn a_CLEAN_trace_is_not_a_pass_and_the_ladder_continues() {
        // The asymmetry that matters: evidence of failure is conclusive, evidence of success is not.
        // Code can run without faulting and still compute the wrong answer, so a clean trace must not
        // short-circuit the way a fault does.
        let mut asked = false;
        let r = walk_traced(false, 0, || { asked = true; v(0.95, 0.9) },
                            || panic!("second not needed"), || panic!("thinker not needed"));
        assert!(asked, "a clean trace still asks a model");
        assert_eq!(r.rung, Rung::DiscipleVerdict);
    }

    #[test]
    fn a_confident_cheap_answer_stops_the_ladder() {
        let r = walk(0, || v(0.95, 0.9), || panic!("second must not run"), || panic!("thinker must not run"));
        assert_eq!(r.rung, Rung::DiscipleVerdict);
        assert_eq!(r.pass, Some(true));
        // and the economics: this cost 1 unit instead of 30
        assert!(Rung::DiscipleVerdict.cost() * 10.0 < Rung::Thinker.cost());
    }

    #[test]
    fn two_cheap_judges_DISAGREEING_escalates_even_when_both_are_confident() {
        // The interesting rule. Each is individually sure and they contradict each other, which is
        // precisely the ambiguity the expensive model exists to resolve — averaging them to 0.5 would
        // hide it behind a number that looks merely uncertain.
        let mut called = false;
        let r = walk(0, || v(0.5, 0.9), || v(0.02, 0.9), || { called = true; true });
        assert_eq!(r.rung, Rung::Thinker);
        assert!(called, "the thinker is actually consulted");
        assert!(r.why.contains("DISAGREE"), "{}", r.why);
    }

    #[test]
    fn two_agreeing_disciples_settle_it_without_the_thinker() {
        let r = walk(0, || v(0.55, 0.9), || v(0.92, 0.9), || panic!("thinker must not run"));
        assert_eq!(r.rung, Rung::SecondOpinion);
        assert_eq!(r.pass, Some(true));
        assert!(r.why.contains("two disciples agree"));
    }

    #[test]
    fn a_meaningless_verdict_does_not_count_as_agreement() {
        // A score built on 2% of the mass must not be able to confirm anything, however extreme.
        let r = walk(0, || v(0.5, 0.9), || v(0.99, 0.01), || true);
        assert_eq!(r.rung, Rung::Thinker, "the second opinion had no mass behind it");
    }

    #[test]
    fn empty_input_yields_none_rather_than_a_perfect_score() {
        // Zero tokens is not perfect agreement; it is no evidence, and returning 0.0 mean logprob would
        // read as flawless.
        assert!(agreement(&[], 2.0).is_none());
    }
}
