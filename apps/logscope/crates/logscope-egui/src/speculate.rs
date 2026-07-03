//! speculate.rs — "aggressive coordinator" mode: while the user is still forming a request, fan out
//! background INVESTIGATION LANES that anticipate what they'll need, so that when the real request lands
//! there are already-thought-through leads with context to draw on. Like a person working out what you
//! probably want as you start talking.
//!
//! A *lane* is a tight, small-model-friendly prompt derived from the in-progress context by a fixed
//! ruleset (one template per [`LaneKind`]). Lanes become `dispatch::Task`s (`tasks_from_lanes`) run on
//! spare fleet capacity by a cheap-model `Executor` — that dispatch is the IO seam, reusing the existing
//! machinery. Completed lanes land in a [`LeadCache`]; when the user's actual request arrives,
//! [`LeadCache::match_leads`] surfaces the relevant pre-computed results.
//!
//! Everything here — the ruleset, lane generation with guardrails, the cache, and relevance matching —
//! is pure and unit-tested. No GUI, no kernels.
#![allow(dead_code)]

use crate::dispatch::Task;
use std::collections::HashSet;

/// The tight ruleset. Each kind is one anticipation strategy with its own prompt template.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaneKind {
    /// Questions whose answers would most change how to help.
    Clarify,
    /// The concrete sub-tasks the request will likely break into.
    Decompose,
    /// Specific facts / current values worth looking up now.
    Precheck,
    /// The most likely follow-up questions.
    Anticipate,
}

impl LaneKind {
    pub fn label(self) -> &'static str {
        match self { LaneKind::Clarify => "clarify", LaneKind::Decompose => "decompose",
                     LaneKind::Precheck => "precheck", LaneKind::Anticipate => "anticipate" }
    }
    /// The default lane set, in priority order.
    pub fn all() -> Vec<LaneKind> { vec![LaneKind::Clarify, LaneKind::Decompose, LaneKind::Precheck, LaneKind::Anticipate] }
}

/// One background investigation: a small-model-friendly prompt of a given kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lane { pub id: u64, pub kind: LaneKind, pub prompt: String }

/// How aggressively to speculate.
#[derive(Clone, Debug)]
pub struct SpecConfig {
    pub aggressive: bool,        // master switch — off ⇒ no lanes
    pub max_lanes: usize,        // cap on concurrent investigation lanes
    pub min_context_chars: usize,// don't speculate on near-empty context (nothing to anticipate yet)
    pub kinds: Vec<LaneKind>,    // which strategies to run, in order
}
impl Default for SpecConfig {
    fn default() -> Self { Self { aggressive: false, max_lanes: 4, min_context_chars: 12, kinds: LaneKind::all() } }
}

/// The ruleset: render a tight, bounded, small-model-runnable prompt for `kind` over `ctx`. The prompts
/// ask for short, line-delimited output with no preamble — cheap for a small model and easy to parse.
fn prompt_for(kind: LaneKind, ctx: &str) -> String {
    match kind {
        LaneKind::Clarify =>
            format!("A user just started with: \"{ctx}\".\nList up to 3 short clarifying questions whose answers would most change how to help. One per line, no preamble."),
        LaneKind::Decompose =>
            format!("A user is likely about to ask for help with: \"{ctx}\".\nList up to 4 concrete sub-tasks it will break into. One per line, terse, no preamble."),
        LaneKind::Precheck =>
            format!("For the request hinted by: \"{ctx}\".\nList up to 3 specific facts or current values worth looking up now. One per line, no preamble."),
        LaneKind::Anticipate =>
            format!("After: \"{ctx}\".\nGive the 3 most likely follow-up questions the user will ask. One per line, no preamble."),
    }
}

/// Generate the investigation lanes for the current `context`. Returns empty when speculation is off, the
/// context is too thin to anticipate anything, or no kinds are configured. Caps at `max_lanes` and
/// de-duplicates identical prompts; lane ids are assigned 0..n in priority order.
pub fn investigation_lanes(context: &str, cfg: &SpecConfig) -> Vec<Lane> {
    let ctx = context.trim();
    if !cfg.aggressive || ctx.chars().count() < cfg.min_context_chars { return Vec::new(); }
    let mut lanes = Vec::new();
    let mut seen = HashSet::new();
    for &kind in &cfg.kinds {
        if lanes.len() >= cfg.max_lanes { break; }
        let prompt = prompt_for(kind, ctx);
        if seen.insert(prompt.clone()) {
            lanes.push(Lane { id: lanes.len() as u64, kind, prompt });
        }
    }
    lanes
}

/// Turn lanes into dispatch tasks (payload = the lane prompt). Route the resulting tasks to a Dispatcher
/// seeded with cheap-model members so they run on spare capacity without blocking the main request.
pub fn tasks_from_lanes(lanes: &[Lane]) -> Vec<Task> {
    lanes.iter().map(|l| Task { id: l.id, payload: l.prompt.clone() }).collect()
}

/// A completed investigation lane's result.
#[derive(Clone, Debug)]
pub struct Lead { pub kind: LaneKind, pub prompt: String, pub output: String }

/// Holds the leads produced by background lanes and surfaces the ones relevant to the arriving request.
#[derive(Default)]
pub struct LeadCache { leads: Vec<Lead> }

impl LeadCache {
    pub fn new() -> Self { Self::default() }

    /// Record a completed lane's output (pair the lane that produced it with its `dispatch` result).
    pub fn insert(&mut self, lane: &Lane, output: String) {
        self.leads.push(Lead { kind: lane.kind, prompt: lane.prompt.clone(), output });
    }
    /// Convenience: fold a whole `dispatch` result map back into the cache, matching task ids to lanes.
    pub fn absorb(&mut self, lanes: &[Lane], results: &std::collections::HashMap<u64, String>) {
        for l in lanes {
            if let Some(out) = results.get(&l.id) { self.insert(l, out.clone()); }
        }
    }

    pub fn len(&self) -> usize { self.leads.len() }
    pub fn is_empty(&self) -> bool { self.leads.is_empty() }

    /// Surface the leads relevant to the now-arrived `request`, strongest first, by keyword overlap with
    /// each lead's output. `min_overlap` drops weak matches (use 1 to keep any overlap).
    pub fn match_leads(&self, request: &str, min_overlap: usize) -> Vec<&Lead> {
        let req = keywords(request);
        let mut scored: Vec<(usize, &Lead)> = self.leads.iter()
            .map(|l| (overlap(&req, &keywords(&l.output)), l))
            .filter(|(s, _)| *s >= min_overlap)
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored.into_iter().map(|(_, l)| l).collect()
    }
}

/// Content words (lowercased, length ≥ 4 to skip most stopwords/short tokens) for crude overlap scoring,
/// with a light plural fold (trailing `s`, but not `ss`) so `region`/`regions`, `lead`/`leads` match.
fn keywords(s: &str) -> HashSet<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 4)
        .map(|w| if w.len() > 4 && w.ends_with('s') && !w.ends_with("ss") { w[..w.len() - 1].to_string() } else { w.to_string() })
        .collect()
}
fn overlap(a: &HashSet<String>, b: &HashSet<String>) -> usize { a.intersection(b).count() }

#[cfg(test)]
mod tests {
    use super::*;

    const CTX: &str = "i need to parse a large csv and compute stats per region";

    fn aggressive(max: usize, kinds: Vec<LaneKind>) -> SpecConfig {
        SpecConfig { aggressive: true, max_lanes: max, min_context_chars: 12, kinds }
    }

    #[test]
    fn no_lanes_unless_aggressive_with_enough_context() {
        assert!(investigation_lanes(CTX, &SpecConfig::default()).is_empty(), "off by default");
        assert!(investigation_lanes("hi", &aggressive(4, LaneKind::all())).is_empty(), "context too thin");
        assert!(!investigation_lanes(CTX, &aggressive(4, LaneKind::all())).is_empty(), "fires when aggressive + enough context");
    }

    #[test]
    fn lanes_respect_kinds_order_and_cap() {
        let lanes = investigation_lanes(CTX, &aggressive(2, LaneKind::all()));
        assert_eq!(lanes.len(), 2, "capped at max_lanes");
        assert_eq!(lanes[0].kind, LaneKind::Clarify);
        assert_eq!(lanes[1].kind, LaneKind::Decompose);
        assert_eq!(lanes.iter().map(|l| l.id).collect::<Vec<_>>(), vec![0, 1]);
        assert!(lanes.iter().all(|l| l.prompt.contains("csv")), "each prompt carries the context");
    }

    #[test]
    fn lanes_become_tasks_with_prompt_payloads() {
        let lanes = investigation_lanes(CTX, &aggressive(3, LaneKind::all()));
        let tasks = tasks_from_lanes(&lanes);
        assert_eq!(tasks.len(), lanes.len());
        for (t, l) in tasks.iter().zip(&lanes) {
            assert_eq!(t.id, l.id);
            assert_eq!(t.payload, l.prompt);
        }
    }

    #[test]
    fn lead_cache_surfaces_relevant_leads_first() {
        let lanes = investigation_lanes(CTX, &aggressive(4, LaneKind::all()));
        let mut cache = LeadCache::new();
        // mock outputs that would come back from the background lanes
        cache.insert(&lanes[0], "Which regions matter? What statistics — mean, median? How large is the csv?".into()); // clarify
        cache.insert(&lanes[2], "Look up the available memory and the disk path of the data.".into());                 // precheck

        // the real request arrives — should rank the clarify lead (median/region overlap) first
        let hits = cache.match_leads("compute the median stats per region from the csv", 2);
        assert_eq!(hits.len(), 1, "only the strongly-overlapping lead clears the threshold");
        assert_eq!(hits[0].kind, LaneKind::Clarify);

        // a memory/disk query surfaces the precheck lead instead
        let mem = cache.match_leads("how much memory and disk space is available", 1);
        assert_eq!(mem[0].kind, LaneKind::Precheck, "different request surfaces a different lead");
    }

    #[test]
    fn absorb_folds_a_dispatch_result_map() {
        let lanes = investigation_lanes(CTX, &aggressive(3, LaneKind::all()));
        let mut results = std::collections::HashMap::new();
        results.insert(0u64, "clarify output".to_string());
        results.insert(2u64, "precheck output".to_string()); // lane 1 didn't finish
        let mut cache = LeadCache::new();
        cache.absorb(&lanes, &results);
        assert_eq!(cache.len(), 2, "only finished lanes are cached");
    }
}
