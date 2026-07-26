//! liaison.rs — the one component allowed to talk to the operator in prose.
//!
//! Everything else in the system emits structured facts ([`Event`], [`Residency`]). The Liaison is where
//! those become sentences, and it is the *only* such place: keeping prose behind a single boundary is what
//! lets the rest of the system stay machine-readable, and what makes "does this component narrate?" a
//! question with a yes/no answer rather than a style opinion.
//!
//! The hard constraint is that the Liaison never invents. Every sentence it produces must be derivable
//! from an event or a branch that is actually in the tree. That is not a stylistic aspiration — it is
//! testable, and it is tested: on an empty tree every answer is an explicit "nothing observed", never a
//! plausible-sounding summary. An explainer that fabricates a reassuring status is worse than no explainer,
//! because the operator's whole reason to delegate is that they are not watching.
//!
//! It answers the specific questions an operator actually asks of a long-running system — what is
//! happening, what changed, what is blocked, what should I look at, what can wait — and it adapts
//! verbosity without changing the underlying fact.

use crate::exploration::{Audience, Branch, Event, ExplorationTree, Interrupt, Status, should_interrupt};
use crate::residency::{Residency, gib};

/// What the engine reports about the loaded model, as FACTS for the Liaison to render.
///
/// Deliberately plain strings rather than a dependency on the engine's `archfeat`: the Liaison presents
/// observable state and does not compute it, and keeping the crates decoupled is what stops presentation
/// logic leaking into the loader. The engine knows which architecture features it is missing; this is how
/// it says so.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelStatus {
    pub name: String,
    pub arch: String,
    /// One line per declared architecture gap, already phrased by the engine.
    pub gaps: Vec<String>,
}

impl ModelStatus {
    pub fn is_fully_supported(&self) -> bool { self.gaps.is_empty() }
}

/// Presents system state to the operator. Holds no state of its own beyond the audience — it is a pure
/// function of the observable world, so it cannot drift from what actually happened.
#[derive(Clone, Copy, Debug)]
pub struct Liaison {
    pub audience: Audience,
    /// Confidence below which a decision is escalated rather than taken silently.
    pub review_threshold: f64,
}

impl Default for Liaison {
    fn default() -> Self { Liaison { audience: Audience::Developer, review_threshold: 0.5 } }
}

/// What the Liaison is willing to say, and why it is entitled to say it. Keeping the provenance attached
/// means a claim can always be traced back to the fact that produced it.
#[derive(Clone, Debug, PartialEq)]
pub struct Statement {
    pub text: String,
    /// Ids of the branches/events this claim rests on. Empty means the claim is about absence of data,
    /// which is the only thing the Liaison may say without a supporting fact.
    pub grounds: Vec<String>,
}

impl Statement {
    pub fn new(text: impl Into<String>, grounds: Vec<String>) -> Self { Statement { text: text.into(), grounds } }
    /// A statement about having nothing to report — the honest alternative to inventing a summary.
    fn nothing(text: impl Into<String>) -> Self { Statement { text: text.into(), grounds: Vec::new() } }
    pub fn is_grounded(&self) -> bool { !self.grounds.is_empty() }
}

impl Liaison {
    pub fn new(audience: Audience) -> Self { Liaison { audience, ..Default::default() } }

    /// "What is happening?" — active work only, because finished work is answered by `what_changed`.
    pub fn what_is_happening(&self, t: &ExplorationTree) -> Vec<Statement> {
        let active: Vec<&Branch> = t.branches.iter().filter(|b| b.status == Status::Active).collect();
        if active.is_empty() {
            return vec![Statement::nothing("Nothing is running right now.")];
        }
        active.iter().map(|b| Statement::new(
            match self.audience {
                Audience::Operator => format!("Working on: {}", b.objective),
                _ => format!("[{}] active — {} (confidence {:.2})", b.id, b.objective, b.confidence),
            }, vec![b.id.clone()])).collect()
    }

    /// "Why?" — the assumptions and evidence behind one branch, never a rationalisation invented after the
    /// fact. Returns None for an unknown id rather than guessing.
    pub fn why(&self, t: &ExplorationTree, id: &str) -> Option<Vec<Statement>> {
        let b = t.get(id)?;
        let mut out = vec![Statement::new(format!("Objective: {}", b.objective), vec![b.id.clone()])];
        for a in &b.assumptions { out.push(Statement::new(format!("Assumed: {a}"), vec![b.id.clone()])); }
        for e in &b.evidence { out.push(Statement::new(format!("Evidence: {e}"), vec![b.id.clone()])); }
        if self.audience != Audience::Operator {
            for (k, v) in &b.benchmarks { out.push(Statement::new(format!("Measured {k} = {v}"), vec![b.id.clone()])); }
        }
        Some(out)
    }

    /// "What changed?" — the outcome events since a mark, rendered for this audience.
    pub fn what_changed(&self, t: &ExplorationTree, since: usize) -> Vec<Statement> {
        let evs: Vec<&Event> = t.events.iter().skip(since)
            .filter(|e| matches!(e, Event::BranchMerged { .. } | Event::BranchRejected { .. }
                                  | Event::BranchDeferred { .. } | Event::BenchmarkFinished { .. }))
            .collect();
        if evs.is_empty() { return vec![Statement::nothing("No completed work since you last looked.")]; }
        evs.iter().map(|e| Statement::new(e.render(self.audience), vec![event_subject(e)])).collect()
    }

    /// "What is blocked?" — deferred branches together with the condition that would release them, so the
    /// operator can tell which blockers are theirs to clear.
    pub fn what_is_blocked(&self, t: &ExplorationTree) -> Vec<Statement> {
        self.what_is_blocked_with(t, None)
    }

    /// As above, plus the loaded model's declared architecture gaps.
    ///
    /// A model gap belongs here rather than in a log line nobody reads: "the engine does not implement
    /// sliding-window attention for this architecture" is a blocker on getting correct output, and it is
    /// the operator's decision whether that matters for their context length. Reporting it as *blocked*
    /// rather than as a warning is the difference between an informed choice and a surprise.
    pub fn what_is_blocked_with(&self, t: &ExplorationTree, model: Option<&ModelStatus>) -> Vec<Statement> {
        let mut out: Vec<Statement> = t.branches.iter()
            .filter(|b| matches!(b.status, Status::Deferred { .. }))
            .map(|b| {
                let cond = match &b.status { Status::Deferred { retry_when } => retry_when.clone(), _ => String::new() };
                Statement::new(format!("{} — waiting on: {}", b.objective, cond), vec![b.id.clone()])
            }).collect();
        if let Some(m) = model {
            for g in &m.gaps {
                out.push(Statement::new(format!("{}: {g}", m.name), vec![m.arch.clone()]));
            }
        }
        if out.is_empty() { return vec![Statement::nothing("Nothing is blocked.")]; }
        out
    }

    /// "What should I review?" — only the things that genuinely need a human: irreversible actions and
    /// decisions the system is not confident enough to take alone. Everything else proceeds silently,
    /// which is the entire point of the attention budget.
    pub fn what_should_i_review(&self, t: &ExplorationTree, irreversible: &[&str]) -> Vec<(String, Interrupt)> {
        t.branches.iter()
            .filter(|b| b.status == Status::Active)
            .filter_map(|b| should_interrupt(b, self.review_threshold, irreversible.contains(&b.id.as_str()))
                .map(|i| (b.id.clone(), i)))
            .collect()
    }

    /// "What can wait?" — active work that is confident enough to be left alone.
    pub fn what_can_wait(&self, t: &ExplorationTree, irreversible: &[&str]) -> Vec<Statement> {
        t.branches.iter()
            .filter(|b| b.status == Status::Active)
            .filter(|b| should_interrupt(b, self.review_threshold, irreversible.contains(&b.id.as_str())).is_none())
            .map(|b| Statement::new(format!("{} (proceeding on its own)", b.objective), vec![b.id.clone()]))
            .collect()
    }

    /// "What is improving?" — merged branches with their measured gains. Only measured ones: a branch that
    /// succeeded without a number attached is reported as landed, not as an improvement.
    pub fn what_is_improving(&self, t: &ExplorationTree) -> Vec<Statement> {
        let wins: Vec<&Branch> = t.branches.iter().filter(|b| matches!(b.status, Status::Succeeded { .. })).collect();
        if wins.is_empty() { return vec![Statement::nothing("Nothing has landed yet.")]; }
        wins.iter().map(|b| {
            let gain = match b.status { Status::Succeeded { gain } => gain, _ => 0.0 };
            let text = if gain > 0.0 {
                format!("{} ({:+.0}%)", b.objective, gain * 100.0)
            } else {
                format!("{} (landed; no separate speedup measured)", b.objective)
            };
            Statement::new(text, vec![b.id.clone()])
        }).collect()
    }

    /// "What remains uncertain?" — low-confidence branches and known dead ends, so the operator sees the
    /// shape of what is *not* known rather than only the wins.
    pub fn what_remains_uncertain(&self, t: &ExplorationTree) -> Vec<Statement> {
        let mut out = Vec::new();
        for b in &t.branches {
            if b.status == Status::Active && b.confidence < self.review_threshold {
                out.push(Statement::new(
                    format!("{} — confidence only {:.2}", b.objective, b.confidence), vec![b.id.clone()]));
            }
        }
        for (id, reason) in t.dead_ends() {
            out.push(Statement::new(format!("Ruled out: {reason}"), vec![id.to_string()]));
        }
        if out.is_empty() { out.push(Statement::nothing("Nothing outstanding is in doubt.")); }
        out
    }

    /// The memory picture, per the memory-map spec: where residency actually went, and how much had to
    /// move. `reuse_ratio` is the number that matters — everything else is context for it.
    pub fn memory_report(&self, r: &Residency) -> Vec<String> {
        let mut out = Vec::new();
        match self.audience {
            Audience::Operator => {
                out.push(format!("Using {} of graphics memory.", gib(r.gpu_bytes())));
                if r.bytes_uploaded + r.bytes_reused > 0 {
                    out.push(format!("{:.0}% of what it needed was already loaded.", r.reuse_ratio() * 100.0));
                }
            }
            _ => {
                out.push(format!("device {} | host {} | total {}",
                    gib(r.gpu_bytes()), gib(r.host_bytes()), gib(r.total_bytes())));
                out.push(format!("reuse {:.1}% ({} moved in {} uploads, mean {})",
                    r.reuse_ratio() * 100.0, gib(r.bytes_uploaded), r.uploads,
                    gib(r.mean_upload_bytes() as u64)));
                out.extend(r.memory_map(24));
                if self.audience == Audience::Systems {
                    if let Some(v) = r.evict_candidate() {
                        out.push(format!("next eviction candidate: {} ({}, rebuild cost {:.1}×)",
                            v.name, gib(v.bytes), v.reconstruct_cost));
                    }
                }
            }
        }
        out
    }

    /// A single status pass — what an operator returning after a break should read first. Ordered by what
    /// costs them most to not know: things needing a decision, then blockers, then progress.
    pub fn briefing(&self, t: &ExplorationTree, r: &Residency, irreversible: &[&str]) -> Vec<String> {
        self.briefing_with(t, r, irreversible, None)
    }

    /// As `briefing`, including the loaded model's declared gaps.
    pub fn briefing_with(&self, t: &ExplorationTree, r: &Residency, irreversible: &[&str],
                         model: Option<&ModelStatus>) -> Vec<String> {
        let mut out = Vec::new();
        let review = self.what_should_i_review(t, irreversible);
        if !review.is_empty() {
            out.push(format!("Needs you ({}):", review.len()));
            for (id, i) in &review { out.push(format!("  • [{id}] {}", describe_interrupt(i))); }
        }
        let blocked = self.what_is_blocked_with(t, model);
        if blocked.iter().any(|s| s.is_grounded()) {
            out.push("Blocked:".into());
            for s in blocked.iter().filter(|s| s.is_grounded()) { out.push(format!("  • {}", s.text)); }
        }
        let improving = self.what_is_improving(t);
        if improving.iter().any(|s| s.is_grounded()) {
            out.push("Landed:".into());
            for s in improving.iter().filter(|s| s.is_grounded()) { out.push(format!("  • {}", s.text)); }
        }
        let happening = self.what_is_happening(t);
        if happening.iter().any(|s| s.is_grounded()) {
            out.push("Running:".into());
            for s in happening.iter().filter(|s| s.is_grounded()) { out.push(format!("  • {}", s.text)); }
        }
        if out.is_empty() { out.push("Nothing to report — no work recorded yet.".into()); }
        if self.audience != Audience::Operator {
            out.push("Memory:".into());
            for l in self.memory_report(r) { out.push(format!("  {l}")); }
        }
        out
    }
}

fn describe_interrupt(i: &Interrupt) -> String {
    match i {
        Interrupt::PolicyDecision(w) => format!("policy decision: {w}"),
        Interrupt::Ambiguity(w) => format!("ambiguous: {w}"),
        Interrupt::CompetingObjectives(w) => format!("competing objectives: {}", w.join(" vs ")),
        Interrupt::LowConfidence { what, confidence } => format!("low confidence ({confidence:.2}): {what}"),
        Interrupt::IrreversibleAction(w) => format!("irreversible: {w}"),
    }
}

/// The branch/target an event is about — used as provenance for a statement.
fn event_subject(e: &Event) -> String {
    match e {
        Event::BranchCreated { id, .. } | Event::BranchMerged { id, .. }
        | Event::BranchRejected { id, .. } | Event::BranchDeferred { id, .. }
        | Event::BenchmarkFinished { id, .. } => id.clone(),
        Event::ResourceAnalysis { target, .. } => target.clone(),
        Event::HardwareObservation { device, .. } => device.clone(),
        Event::ModelLoaded { name, .. } => name.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exploration::Branch;
    use crate::residency::{Region, Lifetime};

    fn tree() -> ExplorationTree {
        let mut t = ExplorationTree::new();
        // resolved through resolve() rather than set directly, so the outcome EVENTS exist too —
        // what_changed reads events, and a tree with statuses but no events would be a fiction
        t.add(Branch::new("stream", "Stream cold MoE experts from host RAM")
            .evidence("64 MiB fills measured ~32 GiB/s").confidence(0.85));
        t.resolve("stream", Status::Succeeded { gain: 0.0 });
        t.add(Branch::new("repin", "Correct the pin set from observed heat")
            .evidence("hit rate 0% -> 62% on a wrong pin set").confidence(0.9));
        t.resolve("repin", Status::Succeeded { gain: 0.62 });
        t.add(Branch::new("zstd", "Compress expert weights with zstd")
            .evidence("nibble entropy 3.62 of 4.0 bits"));
        t.resolve("zstd", Status::Failed { reason: "absmax quantization maximizes entropy by design".into() });
        t.add(Branch::new("prefetch", "Cross-layer coupling prefetch"));
        t.resolve("prefetch", Status::Deferred { retry_when: "ring headroom or overlapping fills".into() });
        t.add(Branch::new("f16-scales", "Halve scale storage via f16")
            .confidence(0.3)); // active, low confidence
        t
    }

    fn mem() -> Residency {
        let mut r = Residency::new();
        r.add(Region::new("pinned experts", 13_600_000_000, Lifetime::Permanent, true));
        r.add(Region::new("dense weights", 3_940_000_000, Lifetime::Permanent, true));
        r.add(Region::new("host expert store", 55_000_000_000, Lifetime::Session, false));
        let unit = 64u64 << 20;
        for _ in 0..38 { r.record_upload(unit); }
        for _ in 0..62 { r.record_reuse(unit); }
        r
    }

    #[test]
    fn says_nothing_rather_than_inventing_when_there_is_nothing() {
        let empty = ExplorationTree::new();
        let l = Liaison::new(Audience::Developer);
        // every answer must be an explicit absence, never a plausible-sounding summary
        for ss in [l.what_is_happening(&empty), l.what_changed(&empty, 0),
                   l.what_is_blocked(&empty), l.what_is_improving(&empty),
                   l.what_remains_uncertain(&empty)] {
            assert_eq!(ss.len(), 1);
            assert!(!ss[0].is_grounded(), "ungrounded claim invented: {}", ss[0].text);
        }
        assert!(l.briefing(&empty, &Residency::new(), &[])[0].contains("Nothing to report"));
    }

    #[test]
    fn every_substantive_claim_carries_its_provenance() {
        let t = tree();
        let l = Liaison::new(Audience::Developer);
        let mut all = Vec::new();
        all.extend(l.what_is_happening(&t));
        all.extend(l.what_is_blocked(&t));
        all.extend(l.what_is_improving(&t));
        all.extend(l.what_changed(&t, 0));
        for s in &all {
            assert!(s.is_grounded(), "claim without grounds: {}", s.text);
            // and the ground must be a real branch, not a fabricated id
            for g in &s.grounds { assert!(t.get(g).is_some(), "unknown provenance {g}"); }
        }
    }

    #[test]
    fn why_explains_from_recorded_evidence_only() {
        let t = tree();
        let l = Liaison::new(Audience::Developer);
        let why = l.why(&t, "zstd").expect("known branch");
        assert!(why.iter().any(|s| s.text.contains("3.62")), "cites the measurement it actually has");
        // an unknown branch yields nothing rather than a guess
        assert!(l.why(&t, "no-such-branch").is_none());
    }

    #[test]
    fn blocked_reports_the_condition_that_would_release_it() {
        let t = tree();
        let l = Liaison::new(Audience::Developer);
        let b = l.what_is_blocked(&t);
        assert_eq!(b.len(), 1);
        assert!(b[0].text.contains("ring headroom"), "{}", b[0].text);
    }

    #[test]
    fn a_model_gap_is_reported_as_blocked_not_buried_in_a_log() {
        let t = tree();
        let l = Liaison::new(Audience::Developer);
        let m = ModelStatus {
            name: "gemma-4-12b-it".into(), arch: "gemma4".into(),
            gaps: vec!["gemma4 needs sliding-window attention — NOT implemented (windowed layers attend \
to the whole context instead)".into()],
        };
        assert!(!m.is_fully_supported());
        let b = l.what_is_blocked_with(&t, Some(&m));
        // both the deferred branch AND the model gap appear
        assert!(b.iter().any(|s| s.text.contains("ring headroom")), "branch blocker still listed");
        let gap = b.iter().find(|s| s.text.contains("sliding-window")).expect("model gap listed as blocked");
        assert!(gap.text.starts_with("gemma-4-12b-it:"), "named by model: {}", gap.text);
        assert!(gap.is_grounded(), "a model gap carries its arch as provenance");
        // and it reaches the briefing, which is what an operator actually reads
        let brief = l.briefing_with(&t, &Residency::new(), &[], Some(&m)).join("\n");
        assert!(brief.contains("sliding-window"), "gap must survive into the briefing:\n{brief}");
    }

    #[test]
    fn a_fully_supported_model_adds_no_blockers() {
        let empty = ExplorationTree::new();
        let l = Liaison::new(Audience::Developer);
        let m = ModelStatus { name: "Qwen3-Coder".into(), arch: "qwen3moe".into(), gaps: Vec::new() };
        assert!(m.is_fully_supported());
        let b = l.what_is_blocked_with(&empty, Some(&m));
        // with no branches and no gaps this must still be the honest empty answer, not a fabricated one
        assert_eq!(b.len(), 1);
        assert!(!b[0].is_grounded());
        assert!(b[0].text.contains("Nothing is blocked"));
    }

    #[test]
    fn review_queue_holds_only_what_needs_a_human() {
        let t = tree();
        let l = Liaison::new(Audience::Developer);
        // f16-scales is active at confidence 0.30 → below the 0.5 threshold → escalate
        let review = l.what_should_i_review(&t, &[]);
        assert_eq!(review.len(), 1);
        assert_eq!(review[0].0, "f16-scales");
        assert!(matches!(review[0].1, Interrupt::LowConfidence { .. }));
        // marking it irreversible changes WHY it escalates, not whether
        let irr = l.what_should_i_review(&t, &["f16-scales"]);
        assert!(matches!(irr[0].1, Interrupt::IrreversibleAction(_)));
    }

    #[test]
    fn confident_work_is_left_alone() {
        let mut t = ExplorationTree::new();
        t.add(Branch::new("safe", "swap a kernel variant").confidence(0.9));
        let l = Liaison::new(Audience::Developer);
        assert!(l.what_should_i_review(&t, &[]).is_empty(), "confident + reversible → no interruption");
        assert_eq!(l.what_can_wait(&t, &[]).len(), 1);
    }

    #[test]
    fn improvements_distinguish_measured_gains_from_mere_landings() {
        let t = tree();
        let l = Liaison::new(Audience::Developer);
        let imp = l.what_is_improving(&t);
        assert!(imp.iter().any(|s| s.text.contains("+62%")), "the measured win reports its number");
        assert!(imp.iter().any(|s| s.text.contains("no separate speedup measured")),
                "a landing without a number is not dressed up as a speedup");
    }

    #[test]
    fn uncertainty_includes_dead_ends_not_just_open_questions() {
        let t = tree();
        let l = Liaison::new(Audience::Developer);
        let u = l.what_remains_uncertain(&t);
        assert!(u.iter().any(|s| s.text.contains("Ruled out")), "known-bad is part of the picture");
        assert!(u.iter().any(|s| s.text.contains("confidence only")));
    }

    #[test]
    fn verbosity_adapts_without_changing_the_fact() {
        let t = tree(); let r = mem();
        let op = Liaison::new(Audience::Operator);
        let sys = Liaison::new(Audience::Systems);
        let (o, s) = (op.memory_report(&r), sys.memory_report(&r));
        assert!(s.len() > o.len(), "systems view is strictly richer");
        // both state the same reuse fact, at different precision
        assert!(o.iter().any(|l| l.contains("62%")));
        assert!(s.iter().any(|l| l.contains("62.0%")));
        // only the systems view exposes the eviction scheduler's opinion
        assert!(s.iter().any(|l| l.contains("eviction candidate")));
        assert!(!o.iter().any(|l| l.contains("eviction candidate")));
    }

    #[test]
    fn briefing_leads_with_what_needs_a_decision() {
        let t = tree(); let r = mem();
        let l = Liaison::new(Audience::Developer);
        let b = l.briefing(&t, &r, &[]);
        assert!(b[0].starts_with("Needs you"), "decisions first: {:?}", b.first());
        let joined = b.join("\n");
        // and the rest of the picture is present, in decreasing urgency
        assert!(joined.contains("Blocked:") && joined.contains("Landed:") && joined.contains("Memory:"));
        assert!(joined.find("Blocked:") < joined.find("Landed:"));
    }
}
