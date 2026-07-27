//! correlate.rs — one structured record per generation, so a run can be diagnosed without grepping.
//!
//! Every diagnosis in this project has needed the same handful of facts, scattered across a dozen log
//! lines that have to be read together: what model, on what platform, with which levers, how many submits
//! and from where, how the time split, and what came out. Assembling that by hand is slow and — more
//! importantly — easy to do incompletely, which is how "729 of 4256 submits" went unnoticed for a run.
//!
//! ## What this records that a log line cannot
//!
//! **Whether this run differs from the last one.** The single most useful fact in this project's history
//! was that Llama-4 Scout produced byte-identical output across two GPU drivers and four weight-path
//! fixes — which ruled out the entire GPU path at a stroke. Nobody noticed for several rounds, because
//! noticing required remembering four tokens across days. A hash comparison does it for free.

use std::collections::BTreeMap;

/// One generation, fully described.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Run {
    pub model: String,
    pub adapter: String,
    /// Levers set away from their defaults. Empty means "stock", which is itself worth recording.
    pub levers: Vec<String>,
    pub tokens: u64,
    pub wall_ms: f64,
    pub setup_ms: f64,
    pub sync_ms: f64,
    pub submits: u64,
    /// Submits by cause. Keyed so an unattributed remainder is computable rather than assumed absent.
    pub submit_causes: BTreeMap<String, u64>,
    /// The produced token ids, truncated — enough to compare runs, not enough to be a transcript.
    pub out_head: Vec<u32>,
}

impl Run {
    /// A stable fingerprint of the OUTPUT, for spotting an unchanged result across code changes.
    pub fn output_fingerprint(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for t in &self.out_head {
            h ^= *t as u64;
            h = h.wrapping_mul(0x1000_0000_01b3);
        }
        h
    }

    /// Submits the causes do not explain. A silent shortfall makes a partial answer look complete.
    pub fn unattributed(&self) -> u64 {
        self.submits.saturating_sub(self.submit_causes.values().sum::<u64>())
    }

    pub fn ms_per_token(&self) -> f64 {
        if self.tokens == 0 { 0.0 } else { self.wall_ms / self.tokens as f64 }
    }

    /// Where the time went, as a verdict rather than three numbers.
    pub fn bound_by(&self) -> &'static str {
        if self.wall_ms <= 0.0 { return "nothing measured" }
        let sync = self.sync_ms / self.wall_ms;
        let setup = self.setup_ms / self.wall_ms;
        // Submits per token separates a stall from real GPU work: thousands means the frame is drained
        // constantly, tens means each submit carries work.
        let per_tok = if self.tokens == 0 { 0 } else { self.submits / self.tokens };
        if sync > 0.4 && per_tok > 200 { "sync (frame drained — look at submit causes)" }
        else if sync > 0.4 { "GPU execution (the wait IS the compute)" }
        else if setup > 0.2 { "CPU setup" }
        else { "GPU execution" }
    }

    /// The lines a person actually needs, in one place.
    pub fn report(&self) -> Vec<String> {
        let mut v = vec![
            format!("model    {} on {}", self.model, self.adapter),
            format!("levers   {}", if self.levers.is_empty() { "stock".into() } else { self.levers.join(" ") }),
            format!("output   {} tokens, {:.0} ms/tok, fingerprint {:016x}",
                self.tokens, self.ms_per_token(), self.output_fingerprint()),
            format!("time     wall {:.0} ms = sync {:.0} + setup {:.0} + rest {:.0}  -> bound by {}",
                self.wall_ms, self.sync_ms, self.setup_ms,
                (self.wall_ms - self.sync_ms - self.setup_ms).max(0.0), self.bound_by()),
        ];
        let per_tok = if self.tokens == 0 { 0 } else { self.submits / self.tokens };
        let mut causes: Vec<String> = self.submit_causes.iter().map(|(k, n)| format!("{k} {n}")).collect();
        causes.sort();
        v.push(format!("submits  {} ({}/tok): {}", self.submits, per_tok, causes.join(", ")));
        if self.unattributed() > 0 {
            v.push(format!("         WARNING: {} submits unattributed — a path is untagged",
                self.unattributed()));
        }
        v
    }
}

/// How this run compares with the previous one.
#[derive(Clone, PartialEq, Debug)]
pub enum Verdict {
    /// First run; nothing to compare against.
    First,
    /// The output is byte-identical to the previous run.
    OutputUnchanged { since: String },
    /// The output changed.
    OutputChanged,
}

impl Verdict {
    /// What an unchanged output means when the code between runs DID change.
    pub fn note(&self) -> Option<String> {
        match self {
            Verdict::OutputUnchanged { since } => Some(format!(
                "output is byte-identical to the run on {since}. If the code changed between them, \
whatever was changed is NOT on the path that determines this output — which is a stronger result than \
any single measurement, and easy to miss by hand.")),
            _ => None,
        }
    }
}

/// Compare against the previous run's record.
pub fn compare(current: &Run, previous: Option<(&Run, &str)>) -> Verdict {
    match previous {
        None => Verdict::First,
        Some((p, when)) if p.output_fingerprint() == current.output_fingerprint()
            && !current.out_head.is_empty() =>
            Verdict::OutputUnchanged { since: when.to_string() },
        Some(_) => Verdict::OutputChanged,
    }
}

/// Render a run as one appendable record.
pub fn render(run: &Run, stamp: &str, verdict: &Verdict) -> String {
    let mut s = format!("=== run {stamp} ===\n");
    for l in run.report() { s.push_str("  "); s.push_str(&l); s.push('\n'); }
    if let Some(n) = verdict.note() { s.push_str("  NOTE: "); s.push_str(&n); s.push('\n'); }
    s.push_str(&format!("  tokens   {:?}\n", run.out_head));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(out: &[u32], submits: u64, causes: &[(&str, u64)]) -> Run {
        Run { model: "scout".into(), adapter: "M2 Ultra [Vulkan]".into(), levers: vec![],
              tokens: 4, wall_ms: 4212.0, setup_ms: 7.0, sync_ms: 384.0, submits,
              submit_causes: causes.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
              out_head: out.to_vec() }
    }

    #[test]
    fn an_unchanged_output_across_code_changes_is_flagged_automatically() {
        // The most useful fact in this project's history — Scout producing byte-identical output across
        // two GPU drivers and four weight fixes — took several rounds to notice, because noticing meant
        // remembering four token ids across days. This does it for free.
        let a = run(&[30, 24, 323, 200007], 4256, &[("passes", 613)]);
        let b = run(&[30, 24, 323, 200007], 900, &[("passes", 400)]);
        let v = compare(&b, Some((&a, "2026-07-26")));
        assert!(matches!(v, Verdict::OutputUnchanged { .. }));
        let n = v.note().unwrap();
        assert!(n.contains("NOT on the path that determines this output"),
            "the conclusion has to be spelled out, not left as an observation: {n}");
        // different output is not flagged
        let c = run(&[1, 2, 3], 900, &[]);
        assert_eq!(compare(&c, Some((&a, "x"))), Verdict::OutputChanged);
    }

    #[test]
    fn an_empty_output_does_not_count_as_matching() {
        // Two runs that both produced nothing are not evidence of anything, and flagging them as
        // "unchanged" would fire on every crashed run.
        let a = run(&[], 10, &[]);
        let b = run(&[], 10, &[]);
        assert_eq!(compare(&b, Some((&a, "x"))), Verdict::OutputChanged);
    }

    #[test]
    fn unattributed_submits_are_computed_and_WARNED_about() {
        // The exact failure this exists to prevent: 729 of 4256 attributed, reported as percentages of
        // 729, and the shortfall invisible.
        let r = run(&[1], 4256, &[("passes", 613), ("download", 110), ("end_frame", 6)]);
        assert_eq!(r.unattributed(), 3527);
        assert!(r.report().iter().any(|l| l.contains("3527 submits unattributed")));
    }

    #[test]
    fn fully_attributed_runs_carry_no_warning() {
        let r = run(&[1], 100, &[("passes", 60), ("download", 40)]);
        assert_eq!(r.unattributed(), 0);
        assert!(!r.report().iter().any(|l| l.contains("unattributed")));
    }

    #[test]
    fn the_bound_verdict_distinguishes_a_stall_from_real_work() {
        // High sync with thousands of submits/token is a drained frame; high sync with tens is the GPU
        // actually computing. Same percentage, opposite fixes.
        let mut stall = run(&[1], 4256, &[]);
        stall.sync_ms = 4000.0;
        assert!(stall.bound_by().contains("frame drained"));
        let mut busy = run(&[1], 40, &[]);
        busy.sync_ms = 4000.0;
        assert!(busy.bound_by().contains("the wait IS the compute"));
    }

    #[test]
    fn stock_levers_are_recorded_as_stock_not_as_blank() {
        // "no levers" and "we forgot to record them" look the same if the field is empty.
        let r = run(&[1], 10, &[]);
        assert!(r.report().iter().any(|l| l.contains("levers   stock")));
    }

    #[test]
    fn a_record_is_self_contained() {
        // The point is to paste ONE block, not to correlate a dozen lines by hand.
        let r = run(&[30, 24, 323, 200007], 4256, &[("passes", 613)]);
        let s = render(&r, "2026-07-27T10:00", &Verdict::First);
        for needle in ["scout", "M2 Ultra", "ms/tok", "bound by", "submits", "tokens"] {
            assert!(s.contains(needle), "missing {needle} in:\n{s}");
        }
    }
}
