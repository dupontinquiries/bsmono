//! What a worker may be ASKED for, and what it may hand back.
//!
//! # The wall this is for
//!
//! Compaction is the symptom, not the disease. A run that reads whole files
//! into the prompt will hit the window no matter how cleverly the window is
//! managed — one field log shows 113 compactions in 120 turns, each cutting
//! the middle out of an 11403-token prompt. Fixing the cut only changes which
//! part is lost.
//!
//! The fix is upstream: stop putting whole files in. A question like "where is
//! the pane split decided" wants a grep and forty lines, not eight hundred.
//!
//! # Survey before read
//!
//! The observed default is that the model opens the first plausible file and
//! answers from it. That is not the model choosing badly — nothing asks it to
//! choose. [`plan`] makes the choice explicit and structural: cheap
//! **survey** probes run first and their findings decide which expensive
//! **read** probes are worth spending on.
//!
//! # No probe may return a whole file
//!
//! Every [`Finding`] carries its origin — path and line range — so the queen
//! can re-probe precisely instead of widening. A worker that could return
//! everything would put the run straight back into the window it was trying to
//! stay inside, so the bound is in the TYPE rather than in a worker's
//! instructions.
//!
//! # Relationship to `retrieval`
//!
//! `retrieval` is the acceptance rule for a set of facts already gathered —
//! the Miss/Partial/Hit lattice, and never regressing a pinned hit. This is
//! the vocabulary for gathering them. They compose: the queen plans probes
//! here and accepts their findings there.

/// A question a worker can be given. Ordered cheap to expensive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// Dependency edges for a crate. Cheapest useful thing: names, no code.
    Deps { krate: String, depth: u8 },
    /// Symbol names and signatures in a file. No bodies.
    ///
    /// The survey probe. It answers "is the thing I want even in here" for a
    /// fiftieth of the tokens the file costs, and its findings are what make
    /// a `Slice` worth spending on.
    Outline { path: String },
    /// Literal or regex search. Returns matching lines with their numbers.
    Grep { pattern: String, glob: Option<String>, max_hits: u16 },
    /// One symbol's definition, by name.
    Symbol { name: String, path_hint: Option<String> },
    /// An explicit line range. The only probe that returns arbitrary source,
    /// and the most expensive.
    Slice { path: String, first: u32, last: u32 },
}

/// Rough token cost, for planning before spending.
///
/// Deliberately an ESTIMATE and deliberately pessimistic. The queen needs to
/// order probes and refuse the ones it cannot afford; being exactly right
/// would require running them, which is the thing being budgeted.
pub fn estimate_tokens(p: &Probe) -> u32 {
    match p {
        Probe::Deps { depth, .. } => 40 * (*depth as u32).max(1),
        Probe::Outline { .. } => 300,
        // ~18 tokens a line: the line, its number, and the path.
        Probe::Grep { max_hits, .. } => 40 + 18 * (*max_hits as u32),
        Probe::Symbol { .. } => 250,
        Probe::Slice { first, last, .. } => 20 + 13 * last.saturating_sub(*first).saturating_add(1),
    }
}

/// Does this probe gather evidence about WHERE to look, rather than content?
///
/// Survey probes are run first and unconditionally; read probes are only
/// planned once a survey has justified them.
pub fn is_survey(p: &Probe) -> bool {
    matches!(p, Probe::Deps { .. } | Probe::Outline { .. } | Probe::Grep { .. })
}

/// Something a worker found, with enough provenance to re-probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub path: String,
    /// Inclusive 1-based line range this came from.
    pub first: u32,
    pub last: u32,
    pub text: String,
}

impl Finding {
    pub fn lines(&self) -> u32 {
        self.last.saturating_sub(self.first).saturating_add(1)
    }
}

/// The most lines any single finding may carry.
///
/// A worker asked to "read the file and tell me what matters" will return the
/// file. This is the ceiling that makes that impossible — chosen so a finding
/// is a readable excerpt rather than a chapter, and so a handful of them still
/// fit beside a real question.
pub const MAX_FINDING_LINES: u32 = 120;

/// Why a finding was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// More than [`MAX_FINDING_LINES`].
    TooLong { lines: u32 },
    /// The range is inverted or zero-based — a worker that cannot say where
    /// something came from has not given the queen anything it can re-probe.
    BadRange,
    /// Would take the budget past its ceiling.
    OverBudget { would_cost: u32, remaining: u32 },
}

/// A running token budget for one question.
#[derive(Debug, Clone)]
pub struct Budget {
    ceiling: u32,
    spent: u32,
}

impl Budget {
    pub fn new(ceiling: u32) -> Budget {
        Budget { ceiling, spent: 0 }
    }
    pub fn spent(&self) -> u32 {
        self.spent
    }
    pub fn remaining(&self) -> u32 {
        self.ceiling.saturating_sub(self.spent)
    }

    /// Admit a finding, or say why not.
    ///
    /// The queen calls this; a worker cannot spend on its own behalf. That is
    /// the answer to "who holds the budget" — one holder, checked at one
    /// place, so a fan-out of four workers cannot each spend the whole thing.
    pub fn admit(&mut self, f: &Finding) -> Result<u32, Refused> {
        if f.first == 0 || f.last < f.first {
            return Err(Refused::BadRange);
        }
        let lines = f.lines();
        if lines > MAX_FINDING_LINES {
            return Err(Refused::TooLong { lines });
        }
        let cost = 13 * lines;
        if cost > self.remaining() {
            return Err(Refused::OverBudget { would_cost: cost, remaining: self.remaining() });
        }
        self.spent += cost;
        Ok(cost)
    }
}

/// Order probes for one question under one budget.
///
/// Survey probes first, cheapest first within each group, and nothing that
/// does not fit. Returns what to run, in order.
///
/// A `Slice` is dropped when no survey precedes it — not because slices are
/// bad, but because a slice chosen before any survey is the "opened the first
/// plausible file" behaviour, and that is what this exists to stop. A caller
/// that genuinely knows the range asks for the survey too; it costs 300
/// tokens and it is what makes the slice defensible.
pub fn plan(probes: &[Probe], budget_ceiling: u32) -> Vec<Probe> {
    let mut survey: Vec<&Probe> = probes.iter().filter(|p| is_survey(p)).collect();
    let mut read: Vec<&Probe> = probes.iter().filter(|p| !is_survey(p)).collect();
    survey.sort_by_key(|p| estimate_tokens(p));
    read.sort_by_key(|p| estimate_tokens(p));

    let has_survey = !survey.is_empty();
    let mut out = Vec::new();
    let mut spent = 0u32;
    for p in survey.into_iter().chain(read) {
        if !has_survey && matches!(p, Probe::Slice { .. }) {
            continue;
        }
        let c = estimate_tokens(p);
        if spent + c > budget_ceiling {
            continue;
        }
        spent += c;
        out.push(p.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slice(path: &str, first: u32, last: u32) -> Probe {
        Probe::Slice { path: path.into(), first, last }
    }
    fn finding(first: u32, last: u32) -> Finding {
        Finding { path: "a.rs".into(), first, last, text: "x".into() }
    }

    #[test]
    fn a_survey_costs_a_fraction_of_the_file_it_surveys() {
        // The whole economic argument. If an outline were not much cheaper
        // than reading, surveying first would just be a slower way to read.
        let outline = estimate_tokens(&Probe::Outline { path: "big.rs".into() });
        let whole_file = estimate_tokens(&slice("big.rs", 1, 800));
        assert!(outline * 20 < whole_file,
                "outline {outline} is not much cheaper than reading {whole_file}");
    }

    #[test]
    fn a_slice_with_no_survey_before_it_is_dropped() {
        // "Opened the first plausible file and answered from it" is the
        // reported default behaviour. A slice planned with no survey IS that
        // behaviour, so the planner refuses it.
        let p = plan(&[slice("main.rs", 1, 40)], 10_000);
        assert!(p.is_empty(), "a bare slice was planned: {p:?}");

        // With a survey in front, the same slice is fine.
        let p = plan(&[slice("main.rs", 1, 40),
                       Probe::Outline { path: "main.rs".into() }], 10_000);
        assert_eq!(p.len(), 2);
        assert!(is_survey(&p[0]), "the survey did not run first: {p:?}");
    }

    #[test]
    fn surveys_run_before_reads_regardless_of_cost() {
        // A cheap read must not jump ahead of a survey: the survey is what
        // decides whether the read is worth doing at all, so running them by
        // price alone would defeat the ordering.
        let cheap_read = Probe::Symbol { name: "x".into(), path_hint: None };
        let dear_survey = Probe::Grep { pattern: "x".into(), glob: None, max_hits: 40 };
        assert!(estimate_tokens(&dear_survey) > estimate_tokens(&cheap_read));
        let p = plan(&[cheap_read, dear_survey], 10_000);
        assert!(is_survey(&p[0]), "a read outran a survey: {p:?}");
    }

    #[test]
    fn nothing_over_budget_is_planned() {
        // Both directions: the affordable probe is kept and the unaffordable
        // one is skipped rather than the plan being truncated at the first
        // failure — a huge slice must not hide a cheap outline behind it.
        let p = plan(&[
            Probe::Outline { path: "a.rs".into() },     // 300
            slice("a.rs", 1, 5_000),                    // enormous
        ], 1_000);
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(matches!(p[0], Probe::Outline { .. }));
    }

    #[test]
    fn a_worker_cannot_return_a_whole_file() {
        // The bound is in the type, not in a worker's instructions. A worker
        // asked to "read the file and say what matters" WILL return the file.
        let mut b = Budget::new(100_000);
        let too_big = finding(1, MAX_FINDING_LINES + 1);
        assert_eq!(b.admit(&too_big), Err(Refused::TooLong { lines: MAX_FINDING_LINES + 1 }));
        assert_eq!(b.spent(), 0, "a refused finding still spent budget");
        // And the largest legal one is admitted, so the bound is a ceiling
        // rather than an off-by-one that rejects everything useful.
        assert!(b.admit(&finding(1, MAX_FINDING_LINES)).is_ok());
    }

    #[test]
    fn a_finding_that_cannot_say_where_it_came_from_is_refused() {
        // Provenance is what lets the queen re-probe precisely instead of
        // widening. A finding without it is a paraphrase.
        let mut b = Budget::new(100_000);
        assert_eq!(b.admit(&finding(0, 10)), Err(Refused::BadRange));
        assert_eq!(b.admit(&finding(50, 20)), Err(Refused::BadRange));
        assert_eq!(b.spent(), 0);
    }

    #[test]
    fn the_budget_is_held_in_one_place_so_a_fan_out_cannot_multiply_it() {
        // Four workers on one question share one ceiling. If each held its
        // own, a fan-out of four would spend four times the budget and the
        // window would blow exactly as before.
        let mut b = Budget::new(13 * 100);
        for _ in 0..4 {
            let _ = b.admit(&finding(1, 20)); // 4 x 20 lines = 80
        }
        assert_eq!(b.spent(), 13 * 80);
        // The fifth 20-line finding does not fit in the remaining 20 lines'
        // worth... it does. The sixth does not.
        assert!(b.admit(&finding(1, 20)).is_ok());
        let over = b.admit(&finding(1, 20));
        assert!(matches!(over, Err(Refused::OverBudget { .. })), "{over:?}");
        assert_eq!(b.remaining(), 0);
    }

    #[test]
    fn deps_are_the_cheapest_probe_and_slices_the_dearest() {
        // The ordering the planner relies on. If a slice were ever cheaper
        // than a survey the plan would put reads first and the survey-first
        // rule would be decoration.
        let deps = estimate_tokens(&Probe::Deps { krate: "k".into(), depth: 1 });
        let outline = estimate_tokens(&Probe::Outline { path: "a.rs".into() });
        let big_slice = estimate_tokens(&slice("a.rs", 1, 120));
        assert!(deps < outline, "{deps} !< {outline}");
        assert!(outline < big_slice, "{outline} !< {big_slice}");
    }
}
