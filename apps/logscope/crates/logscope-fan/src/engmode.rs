//! engmode.rs — the STATEFUL THOUGHT GUIDE: a long-horizon engineering mode whose phase is decided
//! by EVIDENCE, not by the model's opinion of its own progress.
//!
//! ## Why this exists
//!
//! `stepverify` established the load-bearing fact: **iteration without verification is resampling,
//! not refinement.** This module takes the next step. Stepwise mode gives a run *rhythm* (pause at
//! step boundaries) and stepverify gives it a *gate* (do not advance on broken code). Neither gives
//! it a *position*: nothing in the loop knows whether the run is still surveying, is mid-patch, has
//! regressed, or is actually finished. So a long run drifts — it re-surveys code it already read,
//! re-patches a file it already fixed, and announces completion because the prose felt conclusive.
//!
//! The fix is a small explicit machine:
//!
//!   Survey → Plan → Patch → Check → {Diagnose → Patch}* → Perf? → Done
//!                                 ↘ Blocked (no progress, evidence says so)
//!
//! ## The rule that makes it worth having
//!
//! **A model saying TASK_COMPLETE does not reach Done.** `Signal::ModelClaimsDone` is accepted only
//! when the last check was clean; otherwise it is recorded as an unsupported claim and the machine
//! routes to Diagnose with that fact in the prompt. This is the same principle as the engine's
//! perf verdicts: a claim without a measurement is not a result. It is also the direct answer to
//! "call out when things get done" — the callout fires on evidence (0 errors, tests green) and can
//! therefore be trusted by whatever is watching.
//!
//! ## Layering (the seams, stated up front)
//!
//!   1. `Signal`  — observations. Produced by whoever runs commands; this module never shells out.
//!   2. `Fsm`     — pure transition + guards. Unit-tested here, no I/O, no model, no egui.
//!   3. `Episode` — the trace, and `PatternStats` over it: what actually moved the error count on
//!                  THIS repo. Feeds back into the prompt as observed fact, not advice.
//!   4. `prompt_for` — per-phase instruction text, data rather than code.
//!
//! Layer 2 is the piece that must not vary; layers 1 and 4 are expected to. The runner that calls
//! cargo and the model lives outside, so it can be swapped (fabric worker, single Engine::run, or a
//! test harness feeding synthetic signals) without touching the machine.

use std::collections::HashMap;

/// Where the run is. Deliberately small: every extra phase is a transition table entry that has to
/// be justified by a different PROMPT, and phases that share a prompt should share a state.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Phase {
    /// Reading before writing. Ends when the model has named the files/symbols it will touch.
    Survey,
    /// Turning the survey into an ordered, checkable list of edits.
    Plan,
    /// Producing an edit. One unit of work; the smaller the better for attribution.
    Patch,
    /// Running the checker. No model tokens are spent here — this is the evidence step.
    Check,
    /// Reading a failure and deciding what the next patch is. Distinct from Patch because the
    /// prompt differs sharply: here the model must explain the cause before proposing an edit.
    Diagnose,
    /// Optional: the work compiles and passes, and the goal was performance. Disassembly lives here.
    Perf,
    /// Evidence-backed completion.
    Done,
    /// Evidence-backed non-completion: repeated cycles without the error count moving.
    Blocked,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Survey => "survey", Phase::Plan => "plan", Phase::Patch => "patch",
            Phase::Check => "check", Phase::Diagnose => "diagnose", Phase::Perf => "perf",
            Phase::Done => "done", Phase::Blocked => "blocked",
        }
    }
    pub fn is_terminal(self) -> bool { matches!(self, Phase::Done | Phase::Blocked) }
    /// Whether this phase spends model tokens. `Check` does not — it runs a command.
    pub fn calls_model(self) -> bool { !matches!(self, Phase::Check | Phase::Done | Phase::Blocked) }
}

/// What the outside world observed. Every variant is something a command produced or a stream
/// contained — nothing here is an inference.
#[derive(Clone, PartialEq, Debug)]
pub enum Signal {
    /// The model named the files/symbols it intends to touch.
    Surveyed { targets: usize },
    /// A plan with this many concrete steps was stated.
    Planned { steps: usize },
    /// An edit was produced and applied cleanly.
    PatchApplied { files: usize },
    /// An edit was produced but did not apply (context missing, ambiguous anchor).
    PatchRejected { reason: String },
    /// The checker ran. This is the only signal that can carry the run to Done.
    Checked { errors: usize, warnings: usize, tests_failed: usize },
    /// A performance measurement landed (timing, disassembly inspection, gate result).
    Measured { improved: bool, note: String },
    /// The model wrote its completion marker. NOT sufficient on its own — see `Fsm::step`.
    ModelClaimsDone,
    /// The operator asked to stop.
    Cancelled,
}

/// One recorded transition, with the error count at the time so the profiler can attribute change.
#[derive(Clone, Debug)]
pub struct Beat {
    pub from: Phase,
    pub signal_name: &'static str,
    pub to: Phase,
    pub errors_after: Option<usize>,
}

fn signal_name(s: &Signal) -> &'static str {
    match s {
        Signal::Surveyed { .. } => "surveyed",
        Signal::Planned { .. } => "planned",
        Signal::PatchApplied { .. } => "patch-applied",
        Signal::PatchRejected { .. } => "patch-rejected",
        Signal::Checked { .. } => "checked",
        Signal::Measured { .. } => "measured",
        Signal::ModelClaimsDone => "claims-done",
        Signal::Cancelled => "cancelled",
    }
}

/// The machine. Pure: `step` is a total function of (phase, signal, history).
#[derive(Clone, Debug)]
pub struct Fsm {
    phase: Phase,
    /// Error count from the most recent `Checked`, if any.
    last_errors: Option<usize>,
    /// Consecutive Check→Diagnose cycles in which the error count did not fall.
    stalled_cycles: u32,
    /// How many cycles without progress before declaring Blocked. A run that cannot move the number
    /// after this many attempts is not going to; saying so beats burning the remaining budget.
    stall_limit: u32,
    /// Unsupported completion claims, surfaced in the prompt so the model sees its own overreach.
    unsupported_done_claims: u32,
    /// Whether the goal is performance work (enables the Perf phase after a clean check).
    perf_goal: bool,
    trace: Vec<Beat>,
}

impl Default for Fsm {
    fn default() -> Self {
        Self { phase: Phase::Survey, last_errors: None, stalled_cycles: 0, stall_limit: 3,
               unsupported_done_claims: 0, perf_goal: false, trace: Vec::new() }
    }
}

impl Fsm {
    pub fn new() -> Self { Self::default() }
    pub fn with_perf_goal(mut self, yes: bool) -> Self { self.perf_goal = yes; self }
    pub fn with_stall_limit(mut self, n: u32) -> Self { self.stall_limit = n.max(1); self }

    pub fn phase(&self) -> Phase { self.phase }
    pub fn trace(&self) -> &[Beat] { &self.trace }
    pub fn last_errors(&self) -> Option<usize> { self.last_errors }
    pub fn unsupported_done_claims(&self) -> u32 { self.unsupported_done_claims }

    /// Advance. Returns the new phase.
    ///
    /// The two rules worth reading the code for:
    ///   * `ModelClaimsDone` reaches `Done` ONLY when the last check was clean. A claim with a
    ///     dirty or absent check routes to Check (get evidence) or Diagnose (explain the failure),
    ///     and is counted.
    ///   * A `Checked` whose error count did not fall counts as a stalled cycle; `stall_limit`
    ///     consecutive stalls reach `Blocked` rather than looping until the budget runs out.
    pub fn step(&mut self, sig: &Signal) -> Phase {
        let from = self.phase;
        let to = match (self.phase, sig) {
            (_, Signal::Cancelled) => Phase::Blocked,

            (Phase::Survey, Signal::Surveyed { .. }) => Phase::Plan,
            (Phase::Plan, Signal::Planned { .. }) => Phase::Patch,

            (Phase::Patch, Signal::PatchApplied { .. }) => Phase::Check,
            // A rejected patch is not a failure of the code under repair — it is a failure of the
            // edit itself, so the model goes back to Patch with the reason, not to Diagnose.
            (Phase::Patch, Signal::PatchRejected { .. }) => Phase::Patch,
            (Phase::Diagnose, Signal::PatchApplied { .. }) => Phase::Check,
            (Phase::Diagnose, Signal::PatchRejected { .. }) => Phase::Diagnose,

            (_, Signal::Checked { errors, tests_failed, .. }) => {
                let clean = *errors == 0 && *tests_failed == 0;
                let improved = match self.last_errors { Some(prev) => *errors < prev, None => true };
                self.last_errors = Some(*errors);
                if clean {
                    self.stalled_cycles = 0;
                    if self.perf_goal { Phase::Perf } else { Phase::Done }
                } else {
                    if improved { self.stalled_cycles = 0; } else { self.stalled_cycles += 1; }
                    if self.stalled_cycles >= self.stall_limit { Phase::Blocked } else { Phase::Diagnose }
                }
            }

            (Phase::Perf, Signal::Measured { .. }) => Phase::Done,
            (_, Signal::Measured { .. }) => self.phase,

            (_, Signal::ModelClaimsDone) => {
                match self.last_errors {
                    // Evidence agrees with the claim.
                    Some(0) if !self.perf_goal => Phase::Done,
                    Some(0) => Phase::Perf,
                    // Claim without a clean check. Do not argue with it, just go get the evidence.
                    Some(_) => { self.unsupported_done_claims += 1; Phase::Diagnose }
                    None => { self.unsupported_done_claims += 1; Phase::Check }
                }
            }

            // Anything else is a signal that does not move this phase; staying put is correct and
            // is recorded, so a run that keeps producing the wrong signal is visible in the trace.
            _ => self.phase,
        };
        self.phase = to;
        self.trace.push(Beat { from, signal_name: signal_name(sig), to, errors_after: self.last_errors });
        to
    }

    /// The evidence-backed completion callout, or None. Deliberately returns the numbers: a callout
    /// nobody can check is the thing this module exists to replace.
    pub fn done_callout(&self) -> Option<String> {
        if self.phase != Phase::Done { return None; }
        let beats = self.trace.len();
        let patches = self.trace.iter().filter(|b| b.signal_name == "patch-applied").count();
        let checks = self.trace.iter().filter(|b| b.signal_name == "checked").count();
        Some(format!(
            "DONE — verified: 0 errors, 0 failing tests on the last check. {patches} patch(es) applied \
across {checks} check(s), {beats} state transitions{}.",
            if self.unsupported_done_claims > 0 {
                format!("; {} earlier completion claim(s) were rejected for lack of a clean check",
                        self.unsupported_done_claims)
            } else { String::new() }))
    }

    /// The evidence-backed non-completion callout.
    pub fn blocked_callout(&self) -> Option<String> {
        if self.phase != Phase::Blocked { return None; }
        Some(match self.last_errors {
            Some(e) => format!(
                "BLOCKED — {} consecutive check(s) without the error count falling; still {e} error(s). \
The next edit should change approach rather than retry: the same class of fix has now failed \
{} times.", self.stalled_cycles, self.stalled_cycles),
            None => "BLOCKED — stopped before any check produced evidence.".into(),
        })
    }
}

/// What actually worked on THIS repo. Not advice — counts over the recorded trace.
///
/// The question "profile what patterns work best on a repo" only has an honest answer if the
/// measure is a number the run produced. Here it is: for each (phase, signal) pair, how often the
/// error count fell afterwards. A pattern that never moves the number is reported as such.
#[derive(Default, Debug)]
pub struct PatternStats {
    /// (from-phase label, signal) → (times seen, times the error count fell)
    pub cells: HashMap<(&'static str, &'static str), (u32, u32)>,
}

impl PatternStats {
    pub fn from_traces<'a>(traces: impl IntoIterator<Item = &'a [Beat]>) -> Self {
        let mut s = Self::default();
        for t in traces {
            let mut prev: Option<usize> = None;
            for b in t {
                let e = s.cells.entry((b.from.label(), b.signal_name)).or_insert((0, 0));
                e.0 += 1;
                if let (Some(p), Some(now)) = (prev, b.errors_after) {
                    if now < p { e.1 += 1; }
                }
                if b.errors_after.is_some() { prev = b.errors_after; }
            }
        }
        s
    }

    /// One line per pattern, best first — suitable for pasting into a prompt as observed fact.
    pub fn report(&self, min_seen: u32) -> Vec<String> {
        let mut v: Vec<_> = self.cells.iter()
            .filter(|(_, (seen, _))| *seen >= min_seen)
            .map(|((ph, sig), (seen, won))| {
                let rate = if *seen > 0 { *won as f32 / *seen as f32 } else { 0.0 };
                (rate, *seen, format!("{ph} + {sig}: reduced errors {won}/{seen} times ({:.0}%)",
                                      rate * 100.0))
            })
            .collect();
        v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal)
                          .then(b.1.cmp(&a.1)));
        v.into_iter().map(|(_, _, s)| s).collect()
    }
}

/// Per-phase instruction text. Data, not logic — swapping the wording must not require touching the
/// machine, and the machine must not need to know what the words say.
pub fn prompt_for(phase: Phase, fsm: &Fsm) -> String {
    let mut s = String::new();
    s.push_str(match phase {
        Phase::Survey => "PHASE: SURVEY. Do not write code yet. Read what exists and report: the exact \
files and symbols you will change, and one sentence on why each. If you cannot name a file, say what \
you need to look at next. End with the list — nothing else.",
        Phase::Plan => "PHASE: PLAN. Turn the survey into an ORDERED list of 2-9 concrete edits. Each \
entry names one file and one change, small enough that a compile can attribute a failure to it. No \
code in this phase.",
        Phase::Patch => "PHASE: PATCH. Produce exactly ONE edit from the plan, as a complete \
replacement of a named region with enough surrounding context to be unambiguous. Do not restate the \
plan and do not bundle several edits: the next step compiles this, and a bundled edit destroys the \
attribution that makes the compile useful.",
        Phase::Diagnose => "PHASE: DIAGNOSE. The check failed. Before proposing any edit, write the \
CAUSE in one or two sentences, quoting the specific diagnostic and the line it points at. Then, and \
only then, give the next single edit. An edit without a stated cause is a resample, and resampling \
has already been tried.",
        Phase::Perf => "PHASE: PERF. The code is correct; now make it fast, and measure rather than \
assert. Read the machine code before and after — see the disassembly guide below — and report the \
specific instruction-level reason for any change you make.",
        Phase::Check => "PHASE: CHECK. (No model call: the checker runs here.)",
        Phase::Done => "PHASE: DONE.",
        Phase::Blocked => "PHASE: BLOCKED.",
    });
    if fsm.unsupported_done_claims > 0 && !phase.is_terminal() {
        s.push_str(&format!("\n\nNOTE: you have declared completion {} time(s) without a clean check. \
Completion is decided by the checker, not by the narrative. Do not declare it again until the last \
check reported zero errors.", fsm.unsupported_done_claims));
    }
    if let Some(e) = fsm.last_errors {
        if e > 0 && !phase.is_terminal() {
            s.push_str(&format!("\n\nSTATE: last check reported {e} error(s)."));
        }
    }
    s
}

/// Disassembly and codegen reasoning, appended in the Perf phase.
///
/// aarch64 first, deliberately: the target here is Apple Silicon under Asahi, and a guide that only
/// teaches x86 mnemonics is useless on the machine the work actually runs on.
pub const ASM_GUIDE: &str = "\n\nDISASSEMBLY GUIDE (read the machine code; do not guess at it):\n\
TOOLS: `cargo asm <crate>::<path>::<fn>` for one function's assembly, clean and filtered; \
`cargo rustc -- --emit asm` for a whole .s file; `objdump -d target/release/<bin>` on the built \
artifact; `perf annotate` when you have a profile. Always inspect a RELEASE build — debug assembly \
tells you nothing about shipped performance.\n\
GENERICS: Rust monomorphises, so a generic function has no single body. Name the CONCRETE \
instantiation you want (`my_fn::<f32>`), or you will be told the symbol does not exist. Two \
instantiations of one function are two blocks of machine code, and bloat from that is a real cost \
worth measuring.\n\
WHAT TO LOOK FOR, aarch64 (Apple Silicon, the target here):\n\
  * `bl <panic_bounds_check>` inside a loop — a bounds check was NOT elided. Hoist the check, use \
iterators, or slice once outside the loop.\n\
  * Scalar `fmul`/`fadd` where `fmla` (fused multiply-add) or the `.4s` vector forms should appear — \
the loop did not vectorise. Look for a loop-carried dependency or a non-contiguous access pattern.\n\
  * `ld1`/`st1` with `.4s`/`.2d` lanes — this IS the vectorised form; count them against the scalar \
alternative rather than assuming.\n\
  * Repeated `ldr`/`str` to `[sp, #...]` in the loop body — register spills. Usually too many live \
values; try splitting the loop.\n\
  * `bl` to a function you expected to be inlined — check `#[inline]` and whether the callee crosses \
a crate boundary without LTO.\n\
x86_64 equivalents, when comparing against another machine: `call core::panicking::panic_bounds_check`, \
`vfmadd*` for FMA, `vmovups`/`vaddps` for vectorised loads and arithmetic, `mov qword ptr [rsp+...]` \
churn for spills.\n\
DISCIPLINE: report the instruction-level reason for a change, and re-inspect after the edit. \
'Should be faster' is not a finding; a removed bounds check or a scalar loop turned vector is.";

#[cfg(test)]
mod tests {
    use super::*;

    fn checked(errors: usize) -> Signal { Signal::Checked { errors, warnings: 0, tests_failed: 0 } }

    #[test]
    fn the_happy_path_reaches_done_with_evidence() {
        let mut f = Fsm::new();
        assert_eq!(f.step(&Signal::Surveyed { targets: 2 }), Phase::Plan);
        assert_eq!(f.step(&Signal::Planned { steps: 3 }), Phase::Patch);
        assert_eq!(f.step(&Signal::PatchApplied { files: 1 }), Phase::Check);
        assert_eq!(f.step(&checked(0)), Phase::Done);
        let callout = f.done_callout().expect("done callout");
        assert!(callout.contains("0 errors"), "{callout}");
        assert!(callout.contains("1 patch"), "{callout}");
    }

    /// The rule the whole module exists for.
    #[test]
    fn a_completion_claim_without_a_clean_check_does_not_reach_done() {
        let mut f = Fsm::new();
        f.step(&Signal::Surveyed { targets: 1 });
        f.step(&Signal::Planned { steps: 1 });
        f.step(&Signal::PatchApplied { files: 1 });
        f.step(&checked(4));                     // still broken
        assert_eq!(f.phase(), Phase::Diagnose);
        assert_eq!(f.step(&Signal::ModelClaimsDone), Phase::Diagnose, "a claim must not reach Done");
        assert!(f.done_callout().is_none());
        assert_eq!(f.unsupported_done_claims(), 1);
        assert!(prompt_for(f.phase(), &f).contains("decided by the checker"));
        // And with evidence, the same claim is honoured.
        f.step(&Signal::PatchApplied { files: 1 });
        f.step(&checked(0));
        assert_eq!(f.phase(), Phase::Done);
    }

    /// A claim before ANY check goes and gets the evidence rather than being argued with.
    #[test]
    fn a_claim_with_no_check_at_all_routes_to_check() {
        let mut f = Fsm::new();
        assert_eq!(f.step(&Signal::ModelClaimsDone), Phase::Check);
        assert_eq!(f.unsupported_done_claims(), 1);
    }

    #[test]
    fn repeated_checks_without_progress_reach_blocked() {
        let mut f = Fsm::new().with_stall_limit(3);
        f.step(&Signal::Surveyed { targets: 1 });
        f.step(&Signal::Planned { steps: 1 });
        f.step(&Signal::PatchApplied { files: 1 });
        assert_eq!(f.step(&checked(5)), Phase::Diagnose);   // first check: baseline
        f.step(&Signal::PatchApplied { files: 1 });
        assert_eq!(f.step(&checked(5)), Phase::Diagnose);   // stall 1
        f.step(&Signal::PatchApplied { files: 1 });
        assert_eq!(f.step(&checked(5)), Phase::Diagnose);   // stall 2
        f.step(&Signal::PatchApplied { files: 1 });
        assert_eq!(f.step(&checked(5)), Phase::Blocked);    // stall 3 → stop
        let c = f.blocked_callout().expect("blocked callout");
        assert!(c.contains("without the error count falling"), "{c}");
    }

    #[test]
    fn progress_resets_the_stall_counter() {
        let mut f = Fsm::new().with_stall_limit(2);
        f.step(&checked(9));
        f.step(&checked(9));                                // stall 1
        assert_eq!(f.phase(), Phase::Diagnose);
        assert_eq!(f.step(&checked(4)), Phase::Diagnose);   // progress: counter resets
        assert_eq!(f.step(&checked(4)), Phase::Diagnose);   // stall 1 again, not 2
        assert_eq!(f.step(&checked(4)), Phase::Blocked);
    }

    #[test]
    fn a_rejected_patch_returns_to_patch_not_to_diagnose() {
        let mut f = Fsm::new();
        f.step(&Signal::Surveyed { targets: 1 });
        f.step(&Signal::Planned { steps: 2 });
        assert_eq!(f.phase(), Phase::Patch);
        let p = f.step(&Signal::PatchRejected { reason: "anchor not unique".into() });
        assert_eq!(p, Phase::Patch, "a failed EDIT is not a failed FIX");
    }

    #[test]
    fn a_perf_goal_inserts_the_perf_phase_after_a_clean_check() {
        let mut f = Fsm::new().with_perf_goal(true);
        f.step(&Signal::PatchApplied { files: 1 });
        assert_eq!(f.step(&checked(0)), Phase::Perf);
        assert!(prompt_for(Phase::Perf, &f).contains("measure rather than assert"));
        assert_eq!(f.step(&Signal::Measured { improved: true, note: "bounds check gone".into() }), Phase::Done);
    }

    #[test]
    fn cancel_is_terminal_from_anywhere() {
        for start in [Phase::Survey, Phase::Patch, Phase::Diagnose, Phase::Perf] {
            let mut f = Fsm::new();
            f.phase = start;
            assert_eq!(f.step(&Signal::Cancelled), Phase::Blocked);
        }
    }

    #[test]
    fn pattern_stats_credit_only_the_beats_that_moved_the_number() {
        let mut f = Fsm::new().with_stall_limit(9);
        f.step(&Signal::PatchApplied { files: 1 });
        f.step(&checked(6));
        f.step(&Signal::PatchApplied { files: 1 });
        f.step(&checked(2));      // this one improved
        f.step(&Signal::PatchApplied { files: 1 });
        f.step(&checked(2));      // this one did not
        let stats = PatternStats::from_traces([f.trace()]);
        let report = stats.report(1);
        assert!(!report.is_empty());
        let checked_line = report.iter().find(|l| l.contains("+ checked")).expect("a checked pattern");
        assert!(checked_line.contains("1/3") || checked_line.contains("1/2"),
                "only the improving check should be credited: {checked_line}");
    }

    #[test]
    fn the_asm_guide_targets_the_machine_this_runs_on() {
        // aarch64 first is the point: an x86-only guide is useless on Apple Silicon.
        assert!(ASM_GUIDE.contains("aarch64"));
        assert!(ASM_GUIDE.contains("fmla"));
        assert!(ASM_GUIDE.contains("panic_bounds_check"));
        assert!(ASM_GUIDE.contains("monomorphis"), "generics need the concrete instantiation named");
        assert!(ASM_GUIDE.contains("RELEASE"));
    }

    #[test]
    fn check_spends_no_model_tokens() {
        assert!(!Phase::Check.calls_model());
        assert!(Phase::Diagnose.calls_model());
        assert!(!Phase::Done.calls_model());
    }
}
