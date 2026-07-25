//! exploration.rs — the persistent exploration substrate: structured events, a branch tree that never
//! forgets, expected-value scheduling, and an interrupt policy that spends operator attention sparingly.
//!
//! This is the missing middle of the exploration-oriented architecture. The pieces around it already
//! exist: `coordinator.rs` decides WHAT to run, `logscope-fan/explore.rs` compiles candidates,
//! `blackboard.rs` shares findings between branches, `budget.rs` caps spend and cascades cheap gates
//! first. What was missing is the part that makes exploration *compound*:
//!
//!   * **Events, not prose.** Every subsystem emits [`Event`] — a fact with a value, never a narration.
//!     One event feeds many consumers (operator view, metrics, timeline). [`Event::render`] is the ONLY
//!     place prose is produced, and it adapts to the [`Audience`]; the emitters never format for humans.
//!
//!   * **Branches are never discarded.** A [`Branch`] carries objective, assumptions, evidence,
//!     benchmarks, confidence, cost, dependencies, author, and hardware requirements. A failed branch is
//!     as valuable as a successful one — it stops the next attempt from re-walking a dead end, which is
//!     precisely what a fresh session (or a fresh model) would otherwise do. Rejected work stays
//!     searchable ([`ExplorationTree::search`]).
//!
//!   * **Deferred work retries itself.** [`Status::Deferred`] records the CONDITION that would make an
//!     approach viable. When the world changes (a driver ships, a ceiling lifts, headroom appears),
//!     [`ExplorationTree::unblocked_by`] surfaces exactly the branches worth retrying. Without this, a
//!     "blocked on X" finding is lost the moment the session ends.
//!
//!   * **Value, not arrival order.** [`Proposal::expected_value`] = gain × probability ÷ cost, so the
//!     coordinator schedules by what's worth doing rather than what was submitted first.
//!
//! Everything here is pure and unit-tested. The tests are seeded with this project's REAL findings (the
//! Asahi BO-VMA ceiling, the MoE streaming verdict, the measured zstd dead end), because a knowledge
//! store proves itself on real history or not at all.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Who is reading. The same event renders differently per audience — one source, many presentations.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Audience {
    /// Progress only, no jargon.
    Operator,
    /// What changed and why, in engineering terms.
    Developer,
    /// Every number, including the ones that only matter when something is wrong.
    Systems,
}

/// A structured fact emitted by a subsystem. Never a sentence, never internal reasoning — a value another
/// component can act on. Prose exists only in `render`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event {
    BranchCreated { id: String, objective: String },
    BranchMerged { id: String, gain: f64 },
    BranchRejected { id: String, reason: String },
    BranchDeferred { id: String, retry_when: String },
    BenchmarkFinished { id: String, metric: String, value: f64, baseline: f64 },
    /// A measured claim about where time/space goes, with a confidence — the doc's ResourceAnalysis.
    ResourceAnalysis { target: String, estimated_gain: f64, confidence: f64 },
    /// A property of the machine discovered the hard way. These outlive any single run and are the most
    /// expensive kind of knowledge to re-acquire, so they are first-class.
    HardwareObservation { device: String, fact: String, limit: Option<f64> },
    ModelLoaded { name: String, resident_gib: f64 },
}

impl Event {
    /// The single place an event becomes words. Verbosity adapts; the underlying fact does not change.
    pub fn render(&self, a: Audience) -> String {
        match (self, a) {
            (Event::ModelLoaded { name, .. }, Audience::Operator) => format!("Loaded {name}."),
            (Event::ModelLoaded { name, resident_gib }, Audience::Developer) =>
                format!("{name} resident, {resident_gib:.1} GiB on device."),
            (Event::ModelLoaded { name, resident_gib }, Audience::Systems) =>
                format!("{name}: {resident_gib:.2} GiB resident weights, unified memory, no staging copy."),

            (Event::BenchmarkFinished { metric, value, baseline, .. }, Audience::Operator) => {
                let d = pct_change(*baseline, *value);
                if d > 0.0 { format!("{metric} improved {d:.0}%.") } else { format!("{metric} unchanged.") }
            }
            (Event::BenchmarkFinished { id, metric, value, baseline }, _) =>
                format!("[{id}] {metric}: {value:.3} vs {baseline:.3} baseline ({:+.1}%)", pct_change(*baseline, *value)),

            (Event::ResourceAnalysis { target, .. }, Audience::Operator) => format!("Looking at {target}."),
            (Event::ResourceAnalysis { target, estimated_gain, confidence }, _) =>
                format!("{target}: est. gain {:.0}%, confidence {:.2}", estimated_gain * 100.0, confidence),

            (Event::HardwareObservation { fact, .. }, Audience::Operator) => fact.clone(),
            (Event::HardwareObservation { device, fact, limit }, _) => match limit {
                Some(l) => format!("{device}: {fact} (limit {l:.1})"),
                None => format!("{device}: {fact}"),
            },

            (Event::BranchCreated { objective, .. }, Audience::Operator) => format!("Exploring: {objective}"),
            (Event::BranchCreated { id, objective }, _) => format!("[{id}] created: {objective}"),
            (Event::BranchMerged { id, gain }, _) => format!("[{id}] merged ({:+.1}%)", gain * 100.0),
            (Event::BranchRejected { id, reason }, _) => format!("[{id}] rejected: {reason}"),
            (Event::BranchDeferred { id, retry_when }, _) => format!("[{id}] deferred until: {retry_when}"),
        }
    }
}

fn pct_change(baseline: f64, value: f64) -> f64 {
    if baseline == 0.0 { 0.0 } else { (value - baseline) / baseline * 100.0 }
}

/// Where a branch ended up. `Deferred` is the important one: it carries the condition that would revive it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Status {
    Active,
    /// Merged, with the measured relative gain (0.34 = +34%).
    Succeeded { gain: f64 },
    /// Tried and did not work. Kept forever so it is not retried blindly.
    Failed { reason: String },
    /// Not viable *yet*. `retry_when` is matched against world-change descriptions later.
    Deferred { retry_when: String },
}

/// One line of inquiry, with everything needed to judge it later — including by a reader who was not
/// there. The metadata list is the architecture spec's, verbatim in intent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Branch {
    pub id: String,
    pub objective: String,
    pub assumptions: Vec<String>,
    pub evidence: Vec<String>,
    pub benchmarks: Vec<(String, f64)>,
    pub confidence: f64,
    pub estimated_value: f64,
    pub depends_on: Vec<String>,
    pub author: String,
    pub origin_model: String,
    pub hardware: Vec<String>,
    pub status: Status,
    pub parent: Option<String>,
}

impl Branch {
    pub fn new(id: &str, objective: &str) -> Self {
        Branch {
            id: id.into(), objective: objective.into(),
            assumptions: Vec::new(), evidence: Vec::new(), benchmarks: Vec::new(),
            confidence: 0.5, estimated_value: 0.0, depends_on: Vec::new(),
            author: "unknown".into(), origin_model: "unknown".into(), hardware: Vec::new(),
            status: Status::Active, parent: None,
        }
    }
    pub fn evidence(mut self, e: &str) -> Self { self.evidence.push(e.into()); self }
    pub fn assume(mut self, a: &str) -> Self { self.assumptions.push(a.into()); self }
    pub fn bench(mut self, k: &str, v: f64) -> Self { self.benchmarks.push((k.into(), v)); self }
    pub fn confidence(mut self, c: f64) -> Self { self.confidence = c.clamp(0.0, 1.0); self }
    pub fn hardware(mut self, h: &str) -> Self { self.hardware.push(h.into()); self }
    pub fn author(mut self, a: &str) -> Self { self.author = a.into(); self }
    pub fn parent(mut self, p: &str) -> Self { self.parent = Some(p.into()); self }
    pub fn status(mut self, s: Status) -> Self { self.status = s; self }

    /// Text a keyword search can match: objective + evidence + assumptions + status reason.
    fn haystack(&self) -> String {
        let mut s = format!("{} {}", self.objective, self.assumptions.join(" "));
        s.push(' '); s.push_str(&self.evidence.join(" "));
        match &self.status {
            Status::Failed { reason } => { s.push(' '); s.push_str(reason); }
            Status::Deferred { retry_when } => { s.push(' '); s.push_str(retry_when); }
            _ => {}
        }
        s.to_lowercase()
    }
}

/// A unit of proposed work competing for compute. Ranked by expected value, not arrival order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    pub id: String,
    pub objective: String,
    /// Relative improvement if it works (0.25 = +25%).
    pub estimated_gain: f64,
    /// Probability it works at all.
    pub probability: f64,
    /// Cost in whatever unit the coordinator budgets (GPU-minutes here).
    pub cost: f64,
    pub evidence: Vec<String>,
}

impl Proposal {
    /// gain × probability ÷ cost — value per unit of compute. Zero-cost proposals are treated as maximally
    /// efficient rather than dividing by zero.
    pub fn expected_value(&self) -> f64 {
        if self.cost <= 0.0 { return self.estimated_gain * self.probability * f64::INFINITY.min(1e9); }
        self.estimated_gain * self.probability / self.cost
    }
}

/// Rank proposals by expected value, highest first. Ties keep submission order (stable).
pub fn rank_proposals(mut p: Vec<Proposal>) -> Vec<Proposal> {
    p.sort_by(|a, b| b.expected_value().partial_cmp(&a.expected_value()).unwrap_or(std::cmp::Ordering::Equal));
    p
}

/// Reasons worth spending the operator's attention. Everything else proceeds autonomously — the scarce
/// resource is the human, not the compute.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Interrupt {
    PolicyDecision(String),
    Ambiguity(String),
    CompetingObjectives(Vec<String>),
    LowConfidence { what: String, confidence: f64 },
    IrreversibleAction(String),
}

/// Decide whether a branch warrants interrupting the operator. `threshold` is the confidence floor below
/// which a decision should not be made unilaterally.
pub fn should_interrupt(b: &Branch, threshold: f64, irreversible: bool) -> Option<Interrupt> {
    if irreversible { return Some(Interrupt::IrreversibleAction(b.objective.clone())); }
    if b.confidence < threshold {
        return Some(Interrupt::LowConfidence { what: b.objective.clone(), confidence: b.confidence });
    }
    None
}

/// The persistent tree. Nothing is ever removed — `Failed` and `Deferred` branches are the point.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ExplorationTree {
    pub branches: Vec<Branch>,
    pub events: Vec<Event>,
}

impl ExplorationTree {
    pub fn new() -> Self { Self::default() }

    /// Add a branch and emit its creation event.
    pub fn add(&mut self, b: Branch) {
        self.events.push(Event::BranchCreated { id: b.id.clone(), objective: b.objective.clone() });
        self.branches.push(b);
    }

    pub fn get(&self, id: &str) -> Option<&Branch> { self.branches.iter().find(|b| b.id == id) }

    /// Record an outcome, emitting the matching event. The branch stays in the tree regardless.
    pub fn resolve(&mut self, id: &str, status: Status) {
        if let Some(b) = self.branches.iter_mut().find(|b| b.id == id) {
            b.status = status.clone();
        }
        let ev = match status {
            Status::Succeeded { gain } => Event::BranchMerged { id: id.into(), gain },
            Status::Failed { reason } => Event::BranchRejected { id: id.into(), reason },
            Status::Deferred { retry_when } => Event::BranchDeferred { id: id.into(), retry_when },
            Status::Active => return,
        };
        self.events.push(ev);
    }

    pub fn emit(&mut self, e: Event) { self.events.push(e); }

    /// Keyword search across objectives, assumptions, evidence and failure reasons — so a future attempt
    /// can find "did anyone already try this?" before spending anything.
    pub fn search(&self, q: &str) -> Vec<&Branch> {
        let needles: Vec<String> = q.to_lowercase().split_whitespace().map(|s| s.to_string()).collect();
        self.branches.iter()
            .filter(|b| { let h = b.haystack(); needles.iter().all(|n| h.contains(n.as_str())) })
            .collect()
    }

    /// Dead ends: what NOT to retry, and why. The cheapest knowledge in the system.
    pub fn dead_ends(&self) -> Vec<(&str, &str)> {
        self.branches.iter().filter_map(|b| match &b.status {
            Status::Failed { reason } => Some((b.id.as_str(), reason.as_str())),
            _ => None,
        }).collect()
    }

    /// The retry trigger: given a description of something that changed in the world ("driver updated",
    /// "ring headroom available"), return the deferred branches whose condition that description satisfies.
    /// Matching is keyword-overlap on the recorded `retry_when`, so a condition written months earlier
    /// still fires without anyone remembering it exists.
    pub fn unblocked_by(&self, change: &str) -> Vec<&Branch> {
        let c = change.to_lowercase();
        let significant = |w: &str| -> Option<String> {
            let t = w.trim_matches(|ch: char| !ch.is_alphanumeric()).to_lowercase();
            if t.len() > 3 { Some(t) } else { None }
        };
        self.branches.iter().filter(|b| match &b.status {
            Status::Deferred { retry_when } =>
                retry_when.split_whitespace().filter_map(significant).any(|w| c.contains(&w)),
            _ => false,
        }).collect()
    }

    /// Everything learned about the machine itself — the observations most expensive to rediscover.
    pub fn hardware_facts(&self) -> Vec<&Event> {
        self.events.iter().filter(|e| matches!(e, Event::HardwareObservation { .. })).collect()
    }

    /// Total measured gain across merged branches (compounding, not best-of).
    pub fn realized_gain(&self) -> f64 {
        self.branches.iter().filter_map(|b| match b.status {
            Status::Succeeded { gain } => Some(gain), _ => None,
        }).sum()
    }

    /// A compact operator-facing digest: what landed, what's blocked, what's known-bad.
    pub fn digest(&self, a: Audience) -> Vec<String> {
        let mut out = Vec::new();
        for b in &self.branches {
            let line = match &b.status {
                Status::Succeeded { gain } => format!("✓ {} ({:+.0}%)", b.objective, gain * 100.0),
                Status::Failed { reason } => format!("✗ {} — {}", b.objective, reason),
                Status::Deferred { retry_when } => format!("⏸ {} — waiting on {}", b.objective, retry_when),
                Status::Active => format!("… {}", b.objective),
            };
            out.push(line);
        }
        if a != Audience::Operator {
            for e in self.hardware_facts() { out.push(e.render(a)); }
        }
        out
    }

    /// Persist / restore. Knowledge that does not survive the process is not knowledge.
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|e| e.to_string())
    }
    pub fn from_json(s: &str) -> Result<Self, String> {
        serde_json::from_str(s).map_err(|e| e.to_string())
    }
}

/// Group branches by their parent, so a tree can be walked/printed without storing child pointers.
pub fn children_of<'a>(t: &'a ExplorationTree, parent: &str) -> Vec<&'a Branch> {
    t.branches.iter().filter(|b| b.parent.as_deref() == Some(parent)).collect()
}

/// Index branches by status kind, for dashboards.
pub fn status_counts(t: &ExplorationTree) -> HashMap<&'static str, usize> {
    let mut m = HashMap::new();
    for b in &t.branches {
        let k = match b.status {
            Status::Active => "active", Status::Succeeded { .. } => "succeeded",
            Status::Failed { .. } => "failed", Status::Deferred { .. } => "deferred",
        };
        *m.entry(k).or_insert(0) += 1;
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seeded with this project's ACTUAL exploration history, because a knowledge store that only works on
    /// invented examples is not a knowledge store.
    fn real_history() -> ExplorationTree {
        let mut t = ExplorationTree::new();
        t.emit(Event::HardwareObservation {
            device: "Apple M2 Ultra (Asahi/Honeykrisp, Vulkan)".into(),
            fact: "BO-VMA allocation ceiling well below system RAM; Metal has no such ceiling".into(),
            limit: Some(24.0),
        });
        t.add(Branch::new("moe-stream", "Stream cold MoE experts from host RAM to fit past the BO-VMA ceiling")
            .evidence("64 MiB write_buffer fills measured ~32 GiB/s on M2")
            .evidence("per-token compute floor 114 ms; 12 fills/token ~= 23 ms (~20%)")
            .bench("break_even_hit_rate", 0.43)
            .confidence(0.85).hardware("asahi").author("probe")
            .status(Status::Succeeded { gain: 0.0 }));
        t.add(Branch::new("q8-embed", "Quantize the resident embedding f32 -> q8 to reclaim VMA budget")
            .evidence("measured 0.5% RMSE; logits unaffected (separate output projection)")
            .bench("gib_reclaimed", 2.9)
            .confidence(0.95).parent("moe-stream")
            .status(Status::Succeeded { gain: 0.0 }));
        t.add(Branch::new("zstd-experts", "Compress q4 expert weights with zstd before upload")
            .assume("q4 nibbles are compressible like voxel chunk data")
            .evidence("measured nibble entropy 3.62 of 4.0 bits; zlib achieved only 8%")
            .evidence("entropy decode is serial — would erase the 32 GiB/s write_buffer advantage")
            .confidence(0.9)
            .status(Status::Failed { reason: "absmax quantization maximizes nibble entropy by design".into() }));
        t.add(Branch::new("couple-prefetch", "Cross-layer coupling prefetch of experts (Colibri couple_prefetch)")
            .evidence("coupling predictor measured 72% accurate")
            .evidence("capacity-bound not accuracy-bound: displacing variant turned 113 demand fills into 183 total")
            .confidence(0.7)
            .status(Status::Deferred { retry_when: "ring headroom available or fills overlap compute".into() }));
        t
    }

    #[test]
    fn events_render_per_audience_without_changing_the_fact() {
        let e = Event::ModelLoaded { name: "Scout".into(), resident_gib: 22.5 };
        assert_eq!(e.render(Audience::Operator), "Loaded Scout.");
        assert!(e.render(Audience::Developer).contains("22.5 GiB"), "{}", e.render(Audience::Developer));
        assert!(e.render(Audience::Systems).contains("22.50 GiB"), "{}", e.render(Audience::Systems));
        // same fact, three presentations — and the event itself is untouched by rendering
        assert!(e.render(Audience::Systems).len() > e.render(Audience::Operator).len());
        assert_eq!(e, Event::ModelLoaded { name: "Scout".into(), resident_gib: 22.5 });
    }

    #[test]
    fn benchmark_renders_relative_change() {
        let e = Event::BenchmarkFinished { id: "b1".into(), metric: "tok/s".into(), value: 11.8, baseline: 10.0 };
        assert!(e.render(Audience::Operator).contains("18%"));
        assert!(e.render(Audience::Systems).contains("+18.0%"));
    }

    #[test]
    fn failed_branches_are_kept_and_searchable() {
        let t = real_history();
        // the zstd dead end must be findable by someone about to repeat it
        let hits = t.search("zstd");
        assert_eq!(hits.len(), 1);
        assert!(matches!(hits[0].status, Status::Failed { .. }));
        let dead = t.dead_ends();
        assert!(dead.iter().any(|(id, _)| *id == "zstd-experts"));
        // and the reasoning survives, not just the verdict
        assert!(hits[0].evidence.iter().any(|e| e.contains("3.62")));
    }

    #[test]
    fn search_matches_evidence_not_just_objective() {
        let t = real_history();
        // "entropy" appears only in evidence
        let hits = t.search("entropy");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "zstd-experts");
    }

    #[test]
    fn deferred_work_is_revived_by_a_matching_world_change() {
        let t = real_history();
        // nothing to do while the world is unchanged
        assert!(t.unblocked_by("new tokenizer shipped").is_empty());
        // ...but a change that satisfies the recorded condition surfaces the branch, months later,
        // without anyone remembering it existed
        let ready = t.unblocked_by("driver update gives fills that overlap compute");
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "couple-prefetch");
    }

    #[test]
    fn proposals_rank_by_value_per_compute_not_by_gain() {
        let ps = vec![
            Proposal { id: "big-slow".into(), objective: "rewrite".into(),
                       estimated_gain: 0.50, probability: 0.4, cost: 100.0, evidence: vec![] },
            Proposal { id: "small-cheap".into(), objective: "tune".into(),
                       estimated_gain: 0.10, probability: 0.9, cost: 2.0, evidence: vec![] },
        ];
        let ranked = rank_proposals(ps);
        // the smaller gain wins: 0.045/unit vs 0.002/unit
        assert_eq!(ranked[0].id, "small-cheap");
        assert!(ranked[0].expected_value() > ranked[1].expected_value());
    }

    #[test]
    fn interrupts_only_for_the_reasons_that_justify_attention() {
        let confident = Branch::new("x", "swap a kernel").confidence(0.9);
        assert!(should_interrupt(&confident, 0.5, false).is_none(), "confident + reversible → proceed silently");
        assert!(matches!(should_interrupt(&confident, 0.5, true), Some(Interrupt::IrreversibleAction(_))));
        let unsure = Branch::new("y", "change the wire format").confidence(0.2);
        assert!(matches!(should_interrupt(&unsure, 0.5, false), Some(Interrupt::LowConfidence { .. })));
    }

    #[test]
    fn tree_survives_a_round_trip() {
        let t = real_history();
        let json = t.to_json().expect("serialize");
        let back = ExplorationTree::from_json(&json).expect("deserialize");
        assert_eq!(back.branches.len(), t.branches.len());
        // the deferred retry condition — the thing most likely to be lost — survives intact
        let ready = back.unblocked_by("ring headroom is now available");
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "couple-prefetch");
        // as does the hardware ceiling, the most expensive fact to rediscover
        assert_eq!(back.hardware_facts().len(), 1);
    }

    #[test]
    fn resolving_a_branch_emits_an_event_and_keeps_the_branch() {
        let mut t = ExplorationTree::new();
        t.add(Branch::new("b", "try something"));
        let before = t.branches.len();
        t.resolve("b", Status::Failed { reason: "did not build".into() });
        assert_eq!(t.branches.len(), before, "failed branches are never removed");
        assert!(t.events.iter().any(|e| matches!(e, Event::BranchRejected { .. })));
        assert!(matches!(t.get("b").unwrap().status, Status::Failed { .. }));
    }

    #[test]
    fn digest_adapts_and_counts_hold() {
        let t = real_history();
        let op = t.digest(Audience::Operator);
        assert!(op.iter().any(|l| l.starts_with('✗')), "operator still sees dead ends");
        // engineers additionally get the hardware facts
        let sys = t.digest(Audience::Systems);
        assert!(sys.len() > op.len());
        let counts = status_counts(&t);
        assert_eq!(counts.get("succeeded"), Some(&2));
        assert_eq!(counts.get("failed"), Some(&1));
        assert_eq!(counts.get("deferred"), Some(&1));
    }

    #[test]
    fn children_link_to_parents() {
        let t = real_history();
        let kids = children_of(&t, "moe-stream");
        assert_eq!(kids.len(), 1);
        assert_eq!(kids[0].id, "q8-embed");
    }
}
