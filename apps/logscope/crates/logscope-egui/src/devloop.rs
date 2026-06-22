//! devloop.rs — SKETCH of the long-horizon developer-suite orchestrator.
//!
//! This is the missing connective tissue, not new capability: it *sequences* primitives that already
//! exist and are individually validated —
//!     mount::{detect,isolate,snapshot,bundle}   bring a real project into a /tmp scratch, ship a patch
//!     decompose::{find_holes,subtask_prompt,assemble}   split a skeleton into subtasks, refill it
//!     <generation>                              fan K candidate fills per subtask (the model)
//!     sandbox::{verify_files,verify}            build/test each candidate in a bubblewrap /tmp mirror
//!     bestregion::best_region                   pick the largest mostly-passing region
//!     merge::stitch_partial                     stitch winners into the artifact
//! into one autonomous loop with a stop condition, reporting progress so the UI (and the cards queue)
//! can show where it is.
//!
//! Design choices that keep it honest and testable, matching the rest of the crate (fabric's
//! MockWorker, pipeline.rs's seam test):
//!   * The two things devloop must NOT own — the model and the toolchain — sit behind traits
//!     (`Generator`, `Verifier`). Real impls call the engine / `sandbox::verify_*`; tests inject mocks,
//!     so the orchestration logic is exercised without a GPU or a live `cargo`.
//!   * Pure decisions (`pick_winner`, `should_continue`) are free functions with unit tests; `run()` is
//!     the IO-driving shell around them.
//!
//! STATUS: scaffold. The phase sequence and control flow are real; spots that need live wiring to the
//! app's generation path and the mount Workspace internals are marked `SEAM:`. Not yet registered in
//! lib.rs — add `mod devloop;` once the seams are filled against the live signatures.
#![allow(dead_code)]

use crate::bestregion::{best_region, Verdict};
use std::path::{Path, PathBuf};

/// Where the loop is right now — emit through a callback so the UI/cards can render progress without
/// devloop knowing anything about egui.
#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Mounting(String),
    Decomposed { subtasks: usize },
    Proposing { subtask: usize, k: usize },
    Verifying { subtask: usize, candidate: usize },
    Picked { subtask: usize, winner: Option<usize>, verdict: Verdict },
    Retrying { subtask: usize, round: usize },
    Merged { subtasks_done: usize },
    Shipped { patch: PathBuf },
    GaveUp { reason: String },
}

#[derive(Clone, Debug)]
pub struct Config {
    pub scratch_root: PathBuf, // e.g. /tmp/logscope
    pub out_dir: PathBuf,      // where the final patch/bundle lands
    pub candidates: usize,     // K fanned per subtask
    pub max_rounds: usize,     // retries per subtask before giving up (long-horizon bound)
    pub allow_net: bool,       // sandbox network isolation (default false)
}
impl Default for Config {
    fn default() -> Self {
        Self { scratch_root: PathBuf::from("/tmp/logscope"), out_dir: PathBuf::from("."),
               candidates: 4, max_rounds: 3, allow_net: false }
    }
}

/// SEAM to the model. Real impl pushes the prompt onto the chat queue / coordinator and collects K
/// completed branches; the mock returns canned fills.
pub trait Generator {
    /// Propose up to `k` candidate fills for `prompt`. On a retry, `feedback` carries the previous
    /// candidate's compiler error so the model can fix it (the loop's learning signal).
    fn propose(&mut self, prompt: &str, k: usize, feedback: Option<&str>) -> Vec<String>;
}

/// SEAM to the toolchain. Real impl calls `sandbox::verify_files` (bubblewrap /tmp mirror); the mock
/// grades by a predicate. Returns the verdict plus any compiler output to feed back on failure.
pub trait Verifier {
    fn check(&mut self, files: &[(String, String)]) -> (Verdict, String);
}

/// A `Verifier` over the real sandbox: scaffolds the files into a cargo project under `root` and
/// builds them in isolation. (Wiring shown; relies on `sandbox::verify_files`.)
pub struct SandboxVerifier { pub name: String, pub root: PathBuf, pub allow_net: bool }
impl Verifier for SandboxVerifier {
    fn check(&mut self, files: &[(String, String)]) -> (Verdict, String) {
        match crate::sandbox::verify_files(&self.name, files, &self.root, self.allow_net) {
            Ok(r) => (if r.passed { Verdict::Pass } else { Verdict::Fail }, r.output),
            Err(e) => (Verdict::None, format!("verifier error: {e}")),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SubtaskOutcome {
    pub index: usize,
    pub verdict: Verdict,
    pub rounds_used: usize,
    pub fill: Option<String>, // the winning candidate's code, if any passed
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub outcomes: Vec<SubtaskOutcome>,
    pub merged: Option<String>, // the reassembled artifact when every subtask resolved
    pub patch: Option<PathBuf>,
}
impl Report {
    pub fn all_passed(&self) -> bool {
        !self.outcomes.is_empty() && self.outcomes.iter().all(|o| o.verdict == Verdict::Pass)
    }
}

// ── pure decisions (unit-tested) ────────────────────────────────────────────────────────────────

/// Pick the best candidate from `(index, verdict)` pairs: a Pass beats a Partial beats nothing; among
/// equals the earliest wins (FIFO, like the worker pool's admission). None if nothing is usable.
pub fn pick_winner(graded: &[(usize, Verdict)]) -> Option<usize> {
    fn rank(v: Verdict) -> u8 { match v { Verdict::Pass => 3, Verdict::Partial => 2, Verdict::Fail => 1, Verdict::None => 0 } }
    graded.iter().filter(|(_, v)| rank(*v) >= 2).max_by_key(|(_, v)| rank(*v)).map(|&(i, _)| i)
}

/// Keep retrying a failed subtask while rounds remain. (Separated so the budget policy is testable and
/// easy to evolve — e.g. add a wall-clock bound later.)
pub fn should_continue(round: usize, cfg: &Config, last: Verdict) -> bool {
    last != Verdict::Pass && round + 1 < cfg.max_rounds
}

// ── the loop ──────────────────────────────────────────────────────────────────────────────────--

/// Run one subtask to resolution: fan K candidates, verify each, pick a winner; on failure feed the
/// error back and retry until the round budget is spent. Pure w.r.t. IO except through the seams, so
/// this is the unit-testable core of the loop.
fn resolve_subtask<G: Generator, V: Verifier>(
    index: usize, prompt: &str, base_files: &[(String, String)], target: &str,
    cfg: &Config, gen: &mut G, ver: &mut V, mut on: impl FnMut(Phase),
) -> SubtaskOutcome {
    let mut feedback: Option<String> = None;
    let mut last = Verdict::None;
    for round in 0..cfg.max_rounds.max(1) {
        if round > 0 { on(Phase::Retrying { subtask: index, round }); }
        on(Phase::Proposing { subtask: index, k: cfg.candidates });
        let cands = gen.propose(prompt, cfg.candidates, feedback.as_deref());
        let mut graded: Vec<(usize, Verdict)> = Vec::new();
        let mut outputs: Vec<String> = Vec::new();
        for (ci, code) in cands.iter().enumerate() {
            on(Phase::Verifying { subtask: index, candidate: ci });
            // SEAM: place `code` into the target file of a COPY of base_files before verifying.
            let mut files = base_files.to_vec();
            if let Some(slot) = files.iter_mut().find(|(p, _)| p == target) { slot.1 = code.clone(); }
            else { files.push((target.to_string(), code.clone())); }
            let (v, out) = ver.check(&files);
            graded.push((ci, v));
            outputs.push(out);
        }
        let winner = pick_winner(&graded);
        last = winner.map(|i| graded[i].1).unwrap_or(Verdict::Fail);
        on(Phase::Picked { subtask: index, winner, verdict: last });
        if let Some(i) = winner {
            if graded[i].1 == Verdict::Pass {
                return SubtaskOutcome { index, verdict: Verdict::Pass, rounds_used: round + 1, fill: Some(cands[i].clone()) };
            }
        }
        // carry the most informative failure forward as the next round's feedback
        feedback = outputs.into_iter().find(|o| !o.is_empty());
        if !should_continue(round, cfg, last) { break; }
    }
    SubtaskOutcome { index, verdict: last, rounds_used: cfg.max_rounds.max(1), fill: None }
}

/// Full long-horizon run over a `skeleton` whose `decompose::find_holes` define the subtasks. Mount and
/// ship are the boundaries; the middle is decompose → (per subtask: fan/verify/pick/retry) → merge.
///
/// SEAM: `mount_path` is the optional existing project to bring in (None = generate from scratch). When
/// set, call `mount::detect` + `mount::isolate(.., cfg.scratch_root)` first and seed `base_files` from
/// the scratch tree; ship with `mount::bundle` at the end.
pub fn run<G: Generator, V: Verifier>(
    skeleton: &str, base_files: Vec<(String, String)>, target_of: impl Fn(usize) -> String,
    cfg: &Config, gen: &mut G, ver: &mut V, mut on: impl FnMut(Phase),
) -> Report {
    let holes = crate::decompose::find_holes(skeleton);
    on(Phase::Decomposed { subtasks: holes.len() });

    let mut report = Report::default();
    let mut files = base_files;
    for (i, hole) in holes.iter().enumerate() {
        let prompt = crate::decompose::subtask_prompt(skeleton, hole);
        let target = target_of(i);
        let outcome = resolve_subtask(i, &prompt, &files, &target, cfg, gen, ver, &mut on);
        if let Some(fill) = &outcome.fill {
            // accept the winning fill into the working set so later subtasks build on it
            if let Some(slot) = files.iter_mut().find(|(p, _)| *p == target) { slot.1 = fill.clone(); }
            else { files.push((target.clone(), fill.clone())); }
        } else {
            on(Phase::GaveUp { reason: format!("subtask {i} unresolved after {} rounds", outcome.rounds_used) });
        }
        report.outcomes.push(outcome);
    }

    // SEAM: stitch via `decompose::assemble(skeleton, fills)` (and `merge::stitch_partial` when folding
    // into an existing file), then `mount::bundle` to emit the patch into cfg.out_dir.
    let done = report.outcomes.iter().filter(|o| o.verdict == Verdict::Pass).count();
    on(Phase::Merged { subtasks_done: done });
    let _ = (&cfg.out_dir, &skeleton, best_region as fn(&[Option<usize>], &[Verdict], usize, f64) -> _);
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    struct CannedGen { by_round: Vec<Vec<String>>, round: usize }
    impl Generator for CannedGen {
        fn propose(&mut self, _p: &str, _k: usize, _fb: Option<&str>) -> Vec<String> {
            let r = self.by_round.get(self.round).cloned().unwrap_or_default();
            self.round += 1; r
        }
    }
    /// Grades a candidate Pass iff it contains "OK".
    struct KeywordVer;
    impl Verifier for KeywordVer {
        fn check(&mut self, files: &[(String, String)]) -> (Verdict, String) {
            let pass = files.iter().any(|(_, c)| c.contains("OK"));
            if pass { (Verdict::Pass, String::new()) } else { (Verdict::Fail, "error: missing OK".into()) }
        }
    }

    #[test]
    fn pick_winner_prefers_pass_then_partial_then_earliest() {
        assert_eq!(pick_winner(&[(0, Verdict::Fail), (1, Verdict::Partial), (2, Verdict::Pass)]), Some(2));
        assert_eq!(pick_winner(&[(0, Verdict::Partial), (1, Verdict::Partial)]), Some(0), "earliest among equals");
        assert_eq!(pick_winner(&[(0, Verdict::Fail), (1, Verdict::None)]), None, "nothing usable");
    }

    #[test]
    fn should_continue_respects_budget_and_success() {
        let cfg = Config { max_rounds: 3, ..Config::default() };
        assert!(should_continue(0, &cfg, Verdict::Fail));
        assert!(!should_continue(2, &cfg, Verdict::Fail), "out of rounds");
        assert!(!should_continue(0, &cfg, Verdict::Pass), "already passed");
    }

    #[test]
    fn subtask_retries_then_succeeds_on_feedback() {
        // round 0: only failing candidates; round 1: a passing one arrives
        let mut gen = CannedGen { by_round: vec![
            vec!["nope".into(), "still no".into()],
            vec!["now OK".into()],
        ], round: 0 };
        let mut ver = KeywordVer;
        let cfg = Config { candidates: 2, max_rounds: 3, ..Config::default() };
        let mut phases = Vec::new();
        let o = resolve_subtask(0, "fill it", &[("lib.rs".into(), String::new())], "lib.rs",
                                &cfg, &mut gen, &mut ver, |p| phases.push(p));
        assert_eq!(o.verdict, Verdict::Pass);
        assert_eq!(o.rounds_used, 2, "took one retry");
        assert!(o.fill.unwrap().contains("OK"));
        assert!(phases.iter().any(|p| matches!(p, Phase::Retrying { round: 1, .. })));
    }

    #[test]
    fn subtask_gives_up_after_budget() {
        let mut gen = CannedGen { by_round: vec![vec!["bad".into()]; 5], round: 0 };
        let mut ver = KeywordVer;
        let cfg = Config { candidates: 1, max_rounds: 2, ..Config::default() };
        let o = resolve_subtask(0, "x", &[], "lib.rs", &cfg, &mut gen, &mut ver, |_| {});
        assert_eq!(o.verdict, Verdict::Fail);
        assert!(o.fill.is_none());
        assert_eq!(o.rounds_used, 2);
    }
}
