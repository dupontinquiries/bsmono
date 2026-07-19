//! explore.rs — exploratory compilation for the coordinator fan-out. The missing middle between
//! `coordinator.rs` (decides WHAT subtasks to run, best-of-K per subtask) and assembly (`merge.rs`):
//! before stitching, actually *try to build* each candidate and keep the one that compiles.
//!
//! Three ideas, borrowed from the unendlich engine's worker-thread strategy, compound into a superpower:
//!
//!   * **Divide & conquer (Class-P discipline).** Evaluating a candidate is a pure map `&str -> Verdict`
//!     with a disjoint, owned output — the same "Class P" access pattern `util::task_lanes::map_pure`
//!     admits. [`evaluate_batch`] is written to that discipline (order-preserving, no shared mutable
//!     state in the eval), so it stays deterministic and can be swapped to a parallel map — or fanned
//!     out over the fleet via [`unique_tasks`] — without changing results.
//!
//!   * **Batched cache.** Exploratory compilation is expensive (one `rustc` per candidate). The same
//!     snippet shows up again and again — a shared helper across subtasks, an unchanged best-of-K sample
//!     across rounds. [`VerdictCache`] fingerprints content (FNV-1a) and compiles each *distinct*
//!     candidate exactly once, so K samples × N subtasks × R rounds collapses onto the set of things
//!     that are actually different. The cache persists across rounds; a re-run pays only for what changed.
//!
//!   * **Exploratory compilation.** The [`Oracle`] seam is the compile step. Its real impl wraps
//!     `verdict::compile` (rustc / `cargo check`, exit code believed); tests mock it. The winner per
//!     subtask is the highest-*quality* candidate **that actually builds** — a `Verdict::Pass` at low
//!     sampling quality beats a `Verdict::Fail` at high quality, because a thing that doesn't compile is
//!     not a candidate. If nothing builds, the best non-building one is surfaced (flagged) so assembly
//!     still has material and the UI can show what to fix.
//!
//! Everything here is pure + unit-tested. The compiler call and the fleet dispatch are the only seams.
#![allow(dead_code)]

use crate::bestregion::Verdict;
use crate::dispatch::{Executor, Task};
use std::collections::HashMap;

/// One best-of-K sample for a subtask. `quality` is the caller's sampling score (e.g. a
/// `consensus::Consensus` agreement, a logprob, a reranker score) — used only to break ties among
/// candidates that reach the *same* verdict. `content` is the code/answer to be compiled.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub subtask: u64,
    pub content: String,
    pub quality: f32,
}

impl Candidate {
    pub fn new(subtask: u64, content: impl Into<String>, quality: f32) -> Self {
        Self { subtask, content: content.into(), quality }
    }
}

/// The exploratory-compilation seam: content → verdict. Real impl runs the compiler
/// (`verdict::compile(content, cmd, file)`); tests supply a table. Kept `&mut self` so a real oracle
/// may hold a reusable scratch dir / spawn handle.
pub trait Oracle {
    fn evaluate(&mut self, content: &str) -> Verdict;
}

/// The local backing for [`Oracle`]: extract code from a (possibly prose-wrapped) model reply and
/// compile it — the same step `:check` runs, so an exploratory pass grades identically to the oracle
/// the tree already trusts. `cmd` is the compiler line (`verdict::DEFAULT_CHECK`, or a
/// `cargo check -p <crate>` for real-crate grading); `file` is where to drop the code (`None` = a temp
/// `.rs`). When no toolchain is present, `verdict::compile` returns `Verdict::None` rather than faking a
/// pass, so a machine that can't build simply contributes no verdict.
pub struct CompileOracle {
    pub cmd: String,
    pub file: Option<String>,
}

impl CompileOracle {
    pub fn new(cmd: impl Into<String>, file: Option<String>) -> Self {
        Self { cmd: cmd.into(), file }
    }
    /// The default self-contained snippet checker (compile as a lib, metadata only).
    pub fn default_check() -> Self {
        Self { cmd: crate::verdict::DEFAULT_CHECK.to_string(), file: None }
    }
}

impl Oracle for CompileOracle {
    fn evaluate(&mut self, content: &str) -> Verdict {
        // Grade the code the reply carries, not its prose — matches `:check`.
        let code = crate::verdict::extract_code(content);
        crate::verdict::compile(&code, &self.cmd, self.file.as_deref())
    }
}

/// FNV-1a 64-bit over the content bytes — std-only, no dep, stable across runs. The cache key: two
/// byte-identical candidates fingerprint the same and are compiled once.
pub fn fingerprint(content: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in content.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Verdict → rank for ordering: a candidate that builds outranks one that doesn't, always.
/// `Pass(3) > Partial(2) > Fail(1) > None(0)` (`None` = not gradable / no toolchain).
pub fn verdict_rank(v: Verdict) -> u8 {
    match v {
        Verdict::Pass => 3,
        Verdict::Partial => 2,
        Verdict::Fail => 1,
        Verdict::None => 0,
    }
}

/// Batched cache of compile verdicts, keyed by content fingerprint. Compiles each distinct candidate
/// exactly once and remembers it — so repeated candidates (shared helpers, unchanged rounds) are free.
/// Persists across rounds: hand the same cache to successive [`evaluate_batch`] calls.
#[derive(Default)]
pub struct VerdictCache {
    seen: HashMap<u64, Verdict>,
    hits: u64,
    misses: u64,
}

impl VerdictCache {
    pub fn new() -> Self {
        Self { seen: HashMap::new(), hits: 0, misses: 0 }
    }

    /// Verdict for `content`, from cache if seen, else compiled once via `oracle` and stored. Returns
    /// `(verdict, fingerprint)`.
    pub fn get_or_eval<O: Oracle>(&mut self, content: &str, oracle: &mut O) -> (Verdict, u64) {
        let h = fingerprint(content);
        if let Some(&v) = self.seen.get(&h) {
            self.hits += 1;
            return (v, h);
        }
        self.misses += 1;
        let v = oracle.evaluate(content);
        self.seen.insert(h, v);
        (v, h)
    }

    /// A verdict already known for this fingerprint (no compile), if any.
    pub fn peek(&self, content: &str) -> Option<Verdict> {
        self.seen.get(&fingerprint(content)).copied()
    }

    /// Cache hits (candidates served without a compile) and misses (distinct candidates compiled).
    pub fn stats(&self) -> (u64, u64) {
        (self.hits, self.misses)
    }

    /// Distinct candidates compiled so far.
    pub fn distinct(&self) -> usize {
        self.seen.len()
    }
}

/// The verdict for one candidate after (possibly cached) evaluation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Eval {
    pub subtask: u64,
    pub hash: u64,
    pub verdict: Verdict,
}

/// Divide & conquer: evaluate every candidate through the batched cache, in input order. Class-P — each
/// eval is a pure `&str -> Verdict` with an owned output, so the output Vec matches `cands` position for
/// position and the result is independent of evaluation order (safe to parallelize / dispatch).
pub fn evaluate_batch<O: Oracle>(
    cands: &[Candidate],
    cache: &mut VerdictCache,
    oracle: &mut O,
) -> Vec<Eval> {
    cands
        .iter()
        .map(|c| {
            let (verdict, hash) = cache.get_or_eval(&c.content, oracle);
            Eval { subtask: c.subtask, hash, verdict }
        })
        .collect()
}

/// The chosen candidate for a subtask.
#[derive(Clone, Debug, PartialEq)]
pub struct Winner {
    pub subtask: u64,
    pub hash: u64,
    pub verdict: Verdict,
    /// `true` iff the winner actually compiled (`Verdict::Pass`). When no candidate for a subtask built,
    /// the best non-building one wins with `built = false` — assembly still has material, flagged.
    pub built: bool,
}

/// Select one winner per subtask: the candidate with the best `(verdict_rank, quality)`. Compilation is
/// the primary key (a building candidate always beats a non-building one); sampling `quality` only breaks
/// ties within the same verdict. `cands` and `evals` are parallel (same length/order — the output of
/// [`evaluate_batch`] on `cands`). Winners are returned sorted by `subtask`.
pub fn select_winners(cands: &[Candidate], evals: &[Eval]) -> Vec<Winner> {
    // best[subtask] = (rank, quality, Winner)
    let mut best: HashMap<u64, (u8, f32, Winner)> = HashMap::new();
    for (c, e) in cands.iter().zip(evals.iter()) {
        let rank = verdict_rank(e.verdict);
        let cand_key = (rank, c.quality);
        let w = Winner { subtask: c.subtask, hash: e.hash, verdict: e.verdict, built: e.verdict == Verdict::Pass };
        match best.get(&c.subtask) {
            Some(&(br, bq, _)) if (br, bq) >= cand_key => {}
            _ => { best.insert(c.subtask, (rank, c.quality, w)); }
        }
    }
    let mut out: Vec<Winner> = best.into_values().map(|(_, _, w)| w).collect();
    out.sort_by_key(|w| w.subtask);
    out
}

/// One-call local exploratory compilation: grade `(subtask, reply, quality)` candidates through a fresh
/// batched cache with `oracle`, returning the per-subtask [`Winner`]s and the cache stats
/// `(hits, misses)` — how many compiles the dedup saved. The convenience entry a runner command uses:
/// hand it the fan's leaves, get back the best-building candidate per subtask.
pub fn grade_replies<O: Oracle>(
    items: &[(u64, String, f32)],
    oracle: &mut O,
) -> (Vec<Winner>, (u64, u64)) {
    let cands: Vec<Candidate> = items.iter().map(|(s, c, q)| Candidate::new(*s, c.clone(), *q)).collect();
    let mut cache = VerdictCache::new();
    let evals = evaluate_batch(&cands, &mut cache, oracle);
    (select_winners(&cands, &evals), cache.stats())
}

/// Fleet fan-out for exploratory compilation: turn candidates into `dispatch::Task`s over their
/// **distinct** contents only — the dedup that makes the fleet compile each unique candidate once. The
/// returned map lets the caller fold fleet results back by fingerprint (`hash → verdict`) and then call
/// [`select_winners`]. Task ids are the fingerprints, so a result names exactly the content it graded.
pub fn unique_tasks(cands: &[Candidate]) -> (Vec<Task>, HashMap<u64, String>) {
    let mut by_hash: HashMap<u64, String> = HashMap::new();
    let mut order: Vec<u64> = Vec::new();
    for c in cands {
        let h = fingerprint(&c.content);
        if !by_hash.contains_key(&h) {
            by_hash.insert(h, c.content.clone());
            order.push(h);
        }
    }
    let tasks = order.iter().map(|&h| Task { id: h, payload: by_hash[&h].clone() }).collect();
    (tasks, by_hash)
}

// ── fleet fan-out: run exploratory compilation across enlisted machines ──────────────────────────--
//
// The wiring the coordinator drives (the transport loop is the only untested seam — sockets need real
// boxes; everything the loop *feeds* and *consumes* is pure and tested here):
//
//   let (tasks, _by_hash) = explore::unique_tasks(&cands);     // distinct candidates → dispatch tasks
//   let mut disp = dispatch::Dispatcher::new(me, &members, tasks, RETRIES);
//   loop { disp.pump(&mut transport); disp.dispatch(&mut transport); if disp.is_complete() { break } }
//   let evals = explore::fleet_evals(&cands, disp.results());  // fleet verdicts → per-candidate Evals
//   let winners = explore::select_winners(&cands, &evals);     // best-building candidate per subtask
//
// Each worker runs an [`OracleExecutor`] (compile the payload, reply with the verdict tag). Because task
// ids ARE content fingerprints, a returned result names exactly the candidate it graded, and the batched
// dedup already happened in `unique_tasks` — the fleet compiles each distinct candidate once, in parallel.

/// Wire encoding of a [`Verdict`] — the string a worker returns and the coordinator parses back.
pub fn verdict_tag(v: Verdict) -> &'static str {
    match v {
        Verdict::Pass => "Pass",
        Verdict::Partial => "Partial",
        Verdict::Fail => "Fail",
        Verdict::None => "None",
    }
}

/// Parse a [`verdict_tag`] back to a [`Verdict`]; anything unrecognized (or a lost/abandoned task) reads
/// as `None` — the lowest rank, so a machine that couldn't grade never beats one that did.
pub fn parse_verdict(tag: &str) -> Verdict {
    match tag.trim() {
        "Pass" => Verdict::Pass,
        "Partial" => Verdict::Partial,
        "Fail" => Verdict::Fail,
        _ => Verdict::None,
    }
}

/// Member-side executor: runs an assigned candidate (the task payload) through an [`Oracle`] and replies
/// with the verdict tag. Generic over the oracle so a real member uses [`CompileOracle`] while tests
/// inject a table. A no-toolchain member yields `None` verdicts (never a fake pass), so the coordinator's
/// rank ordering simply prefers a member that *could* build.
pub struct OracleExecutor<O: Oracle> {
    pub oracle: O,
}

impl<O: Oracle> OracleExecutor<O> {
    pub fn new(oracle: O) -> Self {
        Self { oracle }
    }
}

/// A member-side executor backed by the local compiler.
pub fn compile_executor(cmd: impl Into<String>, file: Option<String>) -> OracleExecutor<CompileOracle> {
    OracleExecutor::new(CompileOracle::new(cmd, file))
}

impl<O: Oracle> Executor for OracleExecutor<O> {
    fn run(&mut self, _task: u64, payload: &str) -> Result<String, String> {
        // A verdict — even `None` — is a successful grade; the coordinator ranks it. (We reserve `Err`
        // for genuine transport/execution faults, which the dispatcher requeues.)
        Ok(verdict_tag(self.oracle.evaluate(payload)).to_string())
    }
}

/// Coordinator-side reassembly: map a completed dispatch's `results` (task id = content fingerprint →
/// verdict tag) back onto the candidates, producing `Eval`s parallel to `cands` for [`select_winners`].
/// A candidate whose fingerprint is absent from `results` (its task was abandoned, or the member never
/// answered) reads as `Verdict::None`.
pub fn fleet_evals(cands: &[Candidate], results: &HashMap<u64, String>) -> Vec<Eval> {
    cands
        .iter()
        .map(|c| {
            let hash = fingerprint(&c.content);
            let verdict = results.get(&hash).map(|t| parse_verdict(t)).unwrap_or(Verdict::None);
            Eval { subtask: c.subtask, hash, verdict }
        })
        .collect()
}

/// Resolve winners back to the candidate CONTENT that won each subtask (matched by fingerprint),
/// in winner order. The bridge from "which candidate won" to the actual reply to assemble/surface.
pub fn winner_content<'a>(winners: &[Winner], cands: &'a [Candidate]) -> Vec<(u64, &'a str)> {
    winners
        .iter()
        .filter_map(|w| {
            cands
                .iter()
                .find(|c| fingerprint(&c.content) == w.hash)
                .map(|c| (w.subtask, c.content.as_str()))
        })
        .collect()
}

/// Fraction of a subtask set that ended up with a building winner — the coordinator's "how well did the
/// fan-out do" signal. `1.0` = every subtask has a compiling piece; `0.0` = nothing built.
pub fn build_rate(winners: &[Winner]) -> f32 {
    if winners.is_empty() {
        return 0.0;
    }
    let built = winners.iter().filter(|w| w.built).count();
    built as f32 / winners.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mock oracle over a content→verdict table; counts REAL evaluations (a cache hit must not call it).
    struct MockOracle<'a> {
        table: &'a [(&'a str, Verdict)],
        calls: u32,
    }
    impl<'a> Oracle for MockOracle<'a> {
        fn evaluate(&mut self, content: &str) -> Verdict {
            self.calls += 1;
            self.table.iter().find(|(k, _)| *k == content).map(|(_, v)| *v).unwrap_or(Verdict::Fail)
        }
    }

    #[test]
    fn fingerprint_is_deterministic_and_content_addressed() {
        assert_eq!(fingerprint("pub fn a(){}"), fingerprint("pub fn a(){}"));
        assert_ne!(fingerprint("pub fn a(){}"), fingerprint("pub fn a(){"));
        // known FNV-1a property: empty input is the offset basis
        assert_eq!(fingerprint(""), 0xcbf2_9ce4_8422_2325);
    }

    #[test]
    fn verdict_rank_orders_building_over_broken() {
        assert!(verdict_rank(Verdict::Pass) > verdict_rank(Verdict::Partial));
        assert!(verdict_rank(Verdict::Partial) > verdict_rank(Verdict::Fail));
        assert!(verdict_rank(Verdict::Fail) > verdict_rank(Verdict::None));
    }

    #[test]
    fn batched_cache_compiles_each_distinct_candidate_once() {
        const SHARED: &str = "pub fn util(){}";
        let table = [
            ("pub fn a(){}", Verdict::Pass),
            ("pub fn a(){", Verdict::Fail),
            (SHARED, Verdict::Pass),
            ("pub fn b()->i32{1}", Verdict::Pass),
            ("pub fn b(){", Verdict::Fail),
        ];
        let cands = vec![
            Candidate::new(0, "pub fn a(){}", 0.5),
            Candidate::new(0, "pub fn a(){", 0.9),
            Candidate::new(0, SHARED, 0.4),
            Candidate::new(1, "pub fn b()->i32{1}", 0.7),
            Candidate::new(1, SHARED, 0.3), // same bytes as subtask 0's third candidate
            Candidate::new(1, "pub fn b(){", 0.95),
        ];
        let mut cache = VerdictCache::new();
        let mut oracle = MockOracle { table: &table, calls: 0 };
        let evals = evaluate_batch(&cands, &mut cache, &mut oracle);

        // 6 candidates, 5 distinct contents → exactly 5 real compiles, 1 cache hit.
        assert_eq!(oracle.calls, 5, "one real compile per distinct candidate");
        assert_eq!(cache.stats(), (1, 5));
        assert_eq!(cache.distinct(), 5);
        // the reused SHARED candidate carries the same fingerprint both times
        assert_eq!(evals[2].hash, evals[4].hash);
        assert_eq!(evals[2].hash, fingerprint(SHARED));
    }

    #[test]
    fn winner_is_best_building_candidate_quality_only_breaks_ties() {
        const SHARED: &str = "pub fn util(){}";
        let table = [
            ("pub fn a(){}", Verdict::Pass),
            ("pub fn a(){", Verdict::Fail),
            (SHARED, Verdict::Pass),
            ("pub fn b()->i32{1}", Verdict::Pass),
            ("pub fn b(){", Verdict::Fail),
        ];
        let cands = vec![
            Candidate::new(0, "pub fn a(){}", 0.5),      // Pass, low quality
            Candidate::new(0, "pub fn a(){", 0.9),       // Fail, HIGH quality — must lose to the Pass
            Candidate::new(0, SHARED, 0.4),              // Pass, lower quality
            Candidate::new(1, "pub fn b()->i32{1}", 0.7),// Pass
            Candidate::new(1, SHARED, 0.3),              // Pass, low quality
            Candidate::new(1, "pub fn b(){", 0.95),      // Fail, highest quality — must lose
        ];
        let mut cache = VerdictCache::new();
        let mut oracle = MockOracle { table: &table, calls: 0 };
        let evals = evaluate_batch(&cands, &mut cache, &mut oracle);
        let winners = select_winners(&cands, &evals);

        assert_eq!(winners.len(), 2);
        // subtask 0: the Pass at q0.5 beats the Fail at q0.9 AND the Pass at q0.4
        assert_eq!(winners[0].subtask, 0);
        assert!(winners[0].built);
        assert_eq!(winners[0].hash, fingerprint("pub fn a(){}"));
        // subtask 1: Pass 'b()->i32{1}' q0.7 beats the SHARED Pass q0.3 and the Fail q0.95
        assert_eq!(winners[1].subtask, 1);
        assert!(winners[1].built);
        assert_eq!(winners[1].hash, fingerprint("pub fn b()->i32{1}"));

        assert_eq!(build_rate(&winners), 1.0);
    }

    #[test]
    fn falls_back_to_best_non_building_when_nothing_compiles() {
        let table = [("x", Verdict::Fail), ("y", Verdict::None)];
        let cands = vec![
            Candidate::new(7, "x", 0.2), // Fail
            Candidate::new(7, "y", 0.9), // None (worse rank) despite higher quality
        ];
        let mut cache = VerdictCache::new();
        let mut oracle = MockOracle { table: &table, calls: 0 };
        let evals = evaluate_batch(&cands, &mut cache, &mut oracle);
        let winners = select_winners(&cands, &evals);

        assert_eq!(winners.len(), 1);
        assert!(!winners[0].built, "no candidate built");
        assert_eq!(winners[0].verdict, Verdict::Fail, "Fail outranks None even at lower quality");
        assert_eq!(build_rate(&winners), 0.0);
    }

    #[test]
    fn grade_replies_one_call_selects_winners_and_reports_savings() {
        let table = [("good", Verdict::Pass), ("broke", Verdict::Fail)];
        let items = vec![
            (0u64, "good".to_string(), 0.3),
            (0u64, "broke".to_string(), 0.9), // higher quality but fails → loses
            (0u64, "good".to_string(), 0.1), // duplicate content → cache hit, no recompile
        ];
        let mut oracle = MockOracle { table: &table, calls: 0 };
        let (winners, (hits, misses)) = grade_replies(&items, &mut oracle);
        assert_eq!(winners.len(), 1);
        assert!(winners[0].built);
        assert_eq!(winners[0].hash, fingerprint("good"));
        assert_eq!(oracle.calls, 2, "3 candidates, 2 distinct → 2 real compiles");
        assert_eq!((hits, misses), (1, 2));
    }

    #[test]
    fn compile_oracle_grades_empty_reply_as_none_without_a_toolchain() {
        // `verdict::compile` returns `None` for empty code and never fakes a pass — so this path is
        // deterministic even where no compiler exists (the sandbox case). A reply with no code extracts
        // to empty and is not graded.
        let mut o = CompileOracle::default_check();
        assert_eq!(o.evaluate(""), Verdict::None);
        assert_eq!(o.evaluate("just prose, no fenced code here"),
                   crate::verdict::compile(&crate::verdict::extract_code("just prose, no fenced code here"),
                                           &o.cmd, o.file.as_deref()),
                   "oracle delegates to verdict::compile on the extracted code");
    }

    #[test]
    fn unique_tasks_dedups_by_content_for_fleet_fanout() {
        const SHARED: &str = "shared";
        let cands = vec![
            Candidate::new(0, "one", 0.0),
            Candidate::new(0, SHARED, 0.0),
            Candidate::new(1, SHARED, 0.0), // dup content, different subtask
            Candidate::new(1, "two", 0.0),
        ];
        let (tasks, by_hash) = unique_tasks(&cands);
        // 4 candidates, 3 distinct contents → 3 tasks
        assert_eq!(tasks.len(), 3);
        assert_eq!(by_hash.len(), 3);
        // task ids are fingerprints and map back to their content
        for t in &tasks {
            assert_eq!(t.id, fingerprint(&t.payload));
            assert_eq!(by_hash[&t.id], t.payload);
        }
        // the shared content appears exactly once in the task set
        assert_eq!(tasks.iter().filter(|t| t.payload == SHARED).count(), 1);
    }

    #[test]
    fn winner_content_resolves_back_to_the_winning_reply() {
        let table = [("pub fn a(){}", Verdict::Pass), ("nope", Verdict::Fail)];
        let cands = vec![
            Candidate::new(0, "pub fn a(){}", 0.5),
            Candidate::new(0, "nope", 0.9),
            Candidate::new(1, "pub fn a(){}", 0.3), // subtask 1 also lands on the same winning content
        ];
        let mut cache = VerdictCache::new();
        let mut oracle = MockOracle { table: &table, calls: 0 };
        let evals = evaluate_batch(&cands, &mut cache, &mut oracle);
        let winners = select_winners(&cands, &evals);
        let content = winner_content(&winners, &cands);
        assert_eq!(content, vec![(0u64, "pub fn a(){}"), (1u64, "pub fn a(){}")]);
    }

    #[test]
    fn verdict_tag_round_trips() {
        for v in [Verdict::Pass, Verdict::Partial, Verdict::Fail, Verdict::None] {
            assert_eq!(parse_verdict(verdict_tag(v)), v);
        }
        assert_eq!(parse_verdict("garbage from the wire"), Verdict::None, "unknown → None (lowest rank)");
        assert_eq!(parse_verdict("  Pass  "), Verdict::Pass, "whitespace tolerated");
    }

    #[test]
    fn oracle_executor_replies_with_the_verdict_tag() {
        let table = [("ok", Verdict::Pass), ("bad", Verdict::Fail)];
        let mut ex = OracleExecutor::new(MockOracle { table: &table, calls: 0 });
        assert_eq!(ex.run(1, "ok"), Ok("Pass".to_string()));
        assert_eq!(ex.run(2, "bad"), Ok("Fail".to_string()));
        assert_eq!(ex.run(3, "unseen"), Ok("Fail".to_string())); // mock defaults unseen → Fail
    }

    #[test]
    fn fleet_evals_maps_results_by_fingerprint_and_defaults_missing_to_none() {
        let cands = vec![
            Candidate::new(0, "alpha", 0.5),
            Candidate::new(0, "beta", 0.9),  // its task will be "abandoned" (absent from results)
            Candidate::new(1, "gamma", 0.7),
        ];
        // results as a completed Dispatcher would expose them: task id = content fingerprint → tag.
        let mut results = HashMap::new();
        results.insert(fingerprint("alpha"), "Pass".to_string());
        results.insert(fingerprint("gamma"), "Fail".to_string());
        // "beta" deliberately missing (member dropped / retries exhausted)

        let evals = fleet_evals(&cands, &results);
        assert_eq!(evals[0].verdict, Verdict::Pass);
        assert_eq!(evals[1].verdict, Verdict::None, "absent fingerprint → None");
        assert_eq!(evals[2].verdict, Verdict::Fail);
        // and it feeds select_winners just like the local path
        let winners = select_winners(&cands, &evals);
        assert_eq!(winners.len(), 2);
        assert!(winners[0].built); // subtask 0: alpha Pass beats beta None
        assert_eq!(winners[0].hash, fingerprint("alpha"));
        assert!(!winners[1].built); // subtask 1: only a Fail
    }

    #[test]
    fn fleet_and_local_paths_agree_on_winners() {
        // The same candidates graded locally (batched cache) and "over the fleet" (executor → results map)
        // must pick the same winners — the fleet is just a distributed way to fill the same verdicts.
        let table = [
            ("pub fn a(){}", Verdict::Pass),
            ("pub fn a(){", Verdict::Fail),
            ("pub fn b()->i32{1}", Verdict::Pass),
        ];
        let cands = vec![
            Candidate::new(0, "pub fn a(){}", 0.4),
            Candidate::new(0, "pub fn a(){", 0.95),
            Candidate::new(1, "pub fn b()->i32{1}", 0.6),
        ];
        // local
        let mut cache = VerdictCache::new();
        let mut local_oracle = MockOracle { table: &table, calls: 0 };
        let local_evals = evaluate_batch(&cands, &mut cache, &mut local_oracle);
        let local_winners = select_winners(&cands, &local_evals);
        // fleet: each distinct candidate run through the executor, results keyed by fingerprint
        let (tasks, _by_hash) = unique_tasks(&cands);
        let mut ex = OracleExecutor::new(MockOracle { table: &table, calls: 0 });
        let mut results = HashMap::new();
        for t in &tasks {
            results.insert(t.id, ex.run(t.id, &t.payload).unwrap());
        }
        let fleet_evals_v = fleet_evals(&cands, &results);
        let fleet_winners = select_winners(&cands, &fleet_evals_v);

        assert_eq!(local_winners, fleet_winners, "local and fleet exploratory compilation agree");
        // and the fleet compiled each DISTINCT candidate exactly once
        assert_eq!(tasks.len(), 3);
    }
}
