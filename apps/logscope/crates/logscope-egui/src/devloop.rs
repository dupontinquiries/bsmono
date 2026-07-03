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
//! STATUS: registered and unit-tested. The phase sequence and control flow are real; `run` decomposes →
//! resolves each subtask (fan/verify/pick/retry) → assembles the artifact. The mount/ship IO shell
//! (`run_mounted`, owning a real `mount::Workspace`) is the remaining `SEAM:` — it does git/FS IO that the
//! mock tests can't cover, so it's wired separately from this pure, testable core.
#![allow(dead_code)]

use crate::bestregion::Verdict;
use std::path::PathBuf;

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
/// builds them in isolation. Gui-gated like `crate::sandbox` (bubblewrap/cargo); the pure orchestration
/// core below builds on every tier and is exercised with mock verifiers in tests.
#[cfg(feature = "gui")]
pub struct SandboxVerifier { pub name: String, pub root: PathBuf, pub allow_net: bool }
#[cfg(feature = "gui")]
impl Verifier for SandboxVerifier {
    fn check(&mut self, files: &[(String, String)]) -> (Verdict, String) {
        match crate::sandbox::verify_files(&self.name, files, &self.root, self.allow_net) {
            Ok(r) => (if r.passed { Verdict::Pass } else { Verdict::Fail }, r.output),
            Err(e) => (Verdict::None, format!("verifier error: {e}")),
        }
    }
}

/// A `Verifier` that judges a candidate IN the mounted workspace: it overlays the candidate files into
/// `ws_dir` and builds+tests the WHOLE real project in a bubblewrap mirror (`sandbox::verify`), so a fill
/// is graded in its true module/dependency context — unlike `SandboxVerifier`, which scaffolds a toy
/// crate from the files alone. Used by `run_mounted`; gui-gated like `crate::sandbox`/`crate::mount`.
#[cfg(feature = "gui")]
pub struct WorkspaceVerifier { pub ws_dir: PathBuf, pub allow_net: bool }
#[cfg(feature = "gui")]
impl Verifier for WorkspaceVerifier {
    fn check(&mut self, files: &[(String, String)]) -> (Verdict, String) {
        for (p, c) in files {
            let dst = self.ws_dir.join(p);
            if let Some(parent) = dst.parent() { let _ = std::fs::create_dir_all(parent); }
            if std::fs::write(&dst, c).is_err() { return (Verdict::None, format!("could not write {p} into workspace")); }
        }
        match crate::sandbox::verify(&crate::sandbox::VerifySpec::cargo_test(self.ws_dir.clone()).with_net(self.allow_net)) {
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
    // highest rank wins; tie broken by earliest index (max_by_key returns the LAST max on ties, so bias
    // the key toward lower indices with -(i))
    graded.iter().filter(|(_, v)| rank(*v) >= 2)
        .max_by_key(|(i, v)| (rank(*v), -(*i as i64)))
        .map(|&(i, _)| i)
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
    assemble_into: impl Fn(&str) -> String, cfg: &Config, gen: &mut G, ver: &mut V, mut on: impl FnMut(Phase),
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
            // Verify the candidate IN CONTEXT: `assemble_into` splices it into the target file (the
            // skeleton with prior winners filled, this hole = `code`, others still todo!()) so a real
            // build sees a valid file, not a bare function body.
            let content = assemble_into(code);
            let mut files = base_files.to_vec();
            if let Some(slot) = files.iter_mut().find(|(p, _)| p == target) { slot.1 = content; }
            else { files.push((target.to_string(), content)); }
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
    let files = base_files; // the target file holds the whole skeleton; candidates are spliced per-hole
    let mut winners: Vec<(usize, String)> = Vec::new(); // (hole index, winning body) — fills accepted so far
    for (i, hole) in holes.iter().enumerate() {
        let prompt = crate::decompose::subtask_prompt(skeleton, hole);
        let target = target_of(i);
        // verification context = skeleton with prior winners + this candidate spliced (other holes stay todo!())
        let prior: Vec<(crate::decompose::Hole, String)> =
            winners.iter().map(|(j, b)| (holes[*j].clone(), b.clone())).collect();
        let this_hole = hole.clone();
        let assemble_into = |cand: &str| {
            let mut f = prior.clone();
            f.push((this_hole.clone(), cand.to_string()));
            crate::decompose::assemble(skeleton, &f)
        };
        let outcome = resolve_subtask(i, &prompt, &files, &target, assemble_into, cfg, gen, ver, &mut on);
        if let Some(body) = &outcome.fill { winners.push((i, body.clone())); }
        else { on(Phase::GaveUp { reason: format!("subtask {i} unresolved after {} rounds", outcome.rounds_used) }); }
        report.outcomes.push(outcome);
    }

    // Stitch the winning fills back into the skeleton — the context-split subtasks reassembled into one
    // artifact. `report.merged` is set only when every hole resolved (a complete, compilable artifact);
    // partial folding (merge::stitch_partial) and shipping a patch (mount::bundle into cfg.out_dir) live
    // in the `run_mounted` IO shell. [SEAM: ship]
    let fills: Vec<(crate::decompose::Hole, String)> = holes.iter().zip(&report.outcomes)
        .filter_map(|(h, o)| o.fill.clone().map(|f| (h.clone(), f)))
        .collect();
    let done = fills.len();
    if done == holes.len() && !holes.is_empty() {
        report.merged = Some(crate::decompose::assemble(skeleton, &fills));
    }
    on(Phase::Merged { subtasks_done: done });
    report
}

/// The IO shell around `run`: bring an existing project into an isolated `/tmp/logscope/{uuid}` scratch,
/// fill it there, and ship a reviewable patch — the user's tree is never touched. `make_verifier` builds
/// the `Verifier` from the freshly-created scratch dir (production: `WorkspaceVerifier`; tests: a mock),
/// since the verifier needs `ws.dir` which only exists after `isolate`. Gui-gated (real git + sandbox).
#[cfg(feature = "gui")]
pub fn run_mounted<G: Generator, V: Verifier>(
    project: &std::path::Path, skeleton_rel: &str, cfg: &Config,
    gen: &mut G, make_verifier: impl FnOnce(&std::path::Path) -> V, mut on: impl FnMut(Phase),
) -> std::io::Result<Report> {
    on(Phase::Mounting(project.display().to_string()));
    let mount = crate::mount::detect(project)?;
    let ws = crate::mount::isolate(&mount, &cfg.scratch_root)?;   // mirror + clean git baseline in scratch
    let base = read_tree(&ws.dir)?;                                // seed base_files from the scratch tree
    let skeleton = base.iter().find(|(p, _)| p == skeleton_rel).map(|(_, c)| c.clone())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound,
            format!("skeleton '{skeleton_rel}' not found in mounted project")))?;
    let mut ver = make_verifier(&ws.dir);
    let target = skeleton_rel.to_string();
    let mut report = run(&skeleton, base, |_| target.clone(), cfg, gen, &mut ver, &mut on);
    if let Some(merged) = report.merged.clone() {
        std::fs::write(ws.dir.join(skeleton_rel), merged)?;       // land the result in the SCRATCH only
        crate::mount::snapshot(&ws, &format!("devloop: filled {skeleton_rel}"))?;
        let arts = crate::mount::bundle(&ws, &cfg.out_dir)?;       // .bundle + .patch for review
        on(Phase::Shipped { patch: arts.patch.clone() });
        report.patch = Some(arts.patch);
    } else {
        on(Phase::GaveUp { reason: "no complete artifact to ship".into() });
    }
    Ok(report)
}

/// Collect the UTF-8 text files under `dir` as `(relpath, content)`, skipping `.git/` and `target/`.
/// Relpaths use `/` separators to match `skeleton_rel`.
#[cfg(feature = "gui")]
fn read_tree(dir: &std::path::Path) -> std::io::Result<Vec<(String, String)>> {
    fn walk(root: &std::path::Path, cur: &std::path::Path, out: &mut Vec<(String, String)>) -> std::io::Result<()> {
        for ent in std::fs::read_dir(cur)? {
            let ent = ent?;
            let name = ent.file_name();
            if name == ".git" || name == "target" { continue; }
            let p = ent.path();
            if p.is_dir() { walk(root, &p, out)?; }
            else if let Ok(rel) = p.strip_prefix(root) {
                if let Ok(text) = std::fs::read_to_string(&p) {     // skip binaries (non-UTF-8) silently
                    out.push((rel.to_string_lossy().replace('\\', "/"), text));
                }
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out)?;
    Ok(out)
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
                                |c| c.to_string(), &cfg, &mut gen, &mut ver, |p| phases.push(p));
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
        let o = resolve_subtask(0, "x", &[], "lib.rs", |c| c.to_string(), &cfg, &mut gen, &mut ver, |_| {});
        assert_eq!(o.verdict, Verdict::Fail);
        assert!(o.fill.is_none());
        assert_eq!(o.rounds_used, 2);
    }

    #[test]
    fn run_assembles_passing_fills_into_artifact() {
        // Two todo!() holes; each subtask's first candidate passes (contains "OK") → the merged artifact
        // has both holes filled and no todo!() left. Exercises decompose → resolve_subtask → assemble.
        let skeleton = "impl C {\n  fn a() -> u32 { todo!() }\n  fn b() -> u32 { todo!() }\n}";
        let mut gen = CannedGen { by_round: vec![
            vec!["7 /*OK*/".into()],   // subtask 0 (round counter is shared across subtasks)
            vec!["9 /*OK*/".into()],   // subtask 1
        ], round: 0 };
        let mut ver = KeywordVer;
        let cfg = Config { candidates: 1, max_rounds: 2, ..Config::default() };
        let report = run(skeleton, vec![("lib.rs".into(), skeleton.into())], |_| "lib.rs".into(),
                         &cfg, &mut gen, &mut ver, |_| {});
        assert!(report.all_passed(), "both subtasks should pass");
        let merged = report.merged.expect("complete artifact assembled");
        assert!(!merged.contains("todo!()"), "all holes filled: {merged}");
        assert!(merged.contains("7 /*OK*/") && merged.contains("9 /*OK*/"));
    }

    #[cfg(feature = "gui")]
    #[test]
    fn run_mounted_isolates_and_ships_without_touching_source() {
        // Real git + FS (like mount.rs's roundtrip test); generation/verification mocked (no model, no
        // cargo). Asserts a patch is produced AND the user's source tree is untouched (the fill only ever
        // lands in the scratch). Requires `git` on PATH.
        let tmp = std::env::temp_dir().join(format!("devloop-it-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let proj = tmp.join("proj");
        std::fs::create_dir_all(proj.join("src")).unwrap();
        std::fs::write(proj.join("Cargo.toml"), "[package]\nname = \"p\"\nversion = \"0.0.0\"\nedition = \"2021\"\n").unwrap();
        std::fs::write(proj.join("src/lib.rs"), "pub fn answer() -> u32 { todo!() }\n").unwrap();

        let mut gen = CannedGen { by_round: vec![vec!["42 // OK".into()]], round: 0 };
        let cfg = Config { scratch_root: tmp.join("scratch"), out_dir: tmp.join("out"),
                           candidates: 1, max_rounds: 2, allow_net: false };
        let report = run_mounted(&proj, "src/lib.rs", &cfg, &mut gen, |_| KeywordVer, |_| {})
            .expect("mounted run");

        assert!(report.all_passed(), "the single hole should fill");
        assert!(report.patch.expect("a patch was shipped").exists(), "patch artifact exists on disk");
        // isolation guarantee: the ORIGINAL project file still has its hole.
        assert!(std::fs::read_to_string(proj.join("src/lib.rs")).unwrap().contains("todo!()"),
                "source tree must not be mutated");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
