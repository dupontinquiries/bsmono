//! coordinator.rs — prompt-mode orchestration PLANS (pure, no model calls, no egui).
//!
//! Answers the "barebones vs coordinator vs aggressive coordinator" selector. Each mode is a PLAN
//! over the SAME loaded base model, run as one or more fabric workers (LocalWorker), assembled purely
//! by prompt engineering — **no custom/finetuned models and no training suite are required**. The
//! "suite" is this planner plus the prompt templates below; the parallelism is the existing fabric.
//!
//! How the modes differ:
//!   * Barebones            — one worker, the goal verbatim. (today's behavior)
//!   * Coordinator          — decompose into subtasks (e.g. one function each); each worker gets the
//!                            shared context + its own spec + the SIBLING units' names/specs so the
//!                            pieces fit (the "partial fill from the other file workers"); results are
//!                            stitched into one file via merge.rs.
//!   * Aggressive coordinator — same fan-out but best-of-K per subtask (the fabric already does
//!                            best-of-K), trading more parallel model runs for higher quality.
//!
//! Execution lives elsewhere: Barebones → `Engine::run` on the single prompt; Coordinator/Aggressive →
//! spawn `plan().workers` on the fabric `Registry` (each a `LocalWorker` over the loaded model, run
//! with `run_tokens`), take best-of `best_of`, then `Aggregation::StitchFiles` → `merge::stitch_partial`
//! to assemble the per-unit outputs onto the shared-context file. This module only decides WHAT to run.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode { Barebones, Coordinator, Aggressive, Replicator, LongHorizon, StatefulGuide, ThinkerLiaison, LiaisonDebate }

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Barebones => "Barebones",
            Mode::Coordinator => "Coordinator",
            Mode::Aggressive => "Aggressive coordinator",
            Mode::Replicator => "Replicator (parallel chats)",
            Mode::LongHorizon => "Long-horizon engineer",
            Mode::StatefulGuide => "Stateful engineer (guided)",
            Mode::ThinkerLiaison => "Thinker + liaison",
            Mode::LiaisonDebate => "Liaison + debate panel",
        }
    }
    pub fn blurb(self) -> &'static str {
        match self {
            Mode::Barebones => "One model, one prompt — fastest, lowest load.",
            Mode::Coordinator => "Fans the task out to parallel workers (one per unit), each aware of the others, then stitches.",
            Mode::Aggressive => "Like Coordinator, plus best-of-K per unit — more parallel runs for higher quality.",
            // The widest option, and the only one where the fan is STEERED rather than planned:
            // Coordinator decomposes one goal, this runs several goals at once and lets you talk to
            // each. It is listed here rather than left behind a command because a mode reachable
            // only by knowing its name is a mode nobody finds.
            Mode::Replicator => "Several independent chats side by side over one workspace — give each \
worker a different job, message one or all of them, and pair any two so each sees the other's latest. \
One weights copy, one KV cache per worker. Opens with :replicate N, or here.",
            // The three below vary WHO REASONS, where the three above vary how wide the fan-out is.
            // Two axes, deliberately in one list because an operator picks one thing.
            Mode::LongHorizon => "One large model, unguided. For runs where nobody is watching and the \
session has to carry itself — it cannot usefully stop and ask, so it does not try.",
            // Same family as LongHorizon — one large model, nobody watching — but the run carries a
            // POSITION as well as a goal, and the position is decided by the checker.
            Mode::StatefulGuide => "Long-horizon, but state-tracked: survey → plan → patch → check → \
diagnose, with the phase decided by compiler output rather than by the model's opinion. It cannot \
declare itself finished without a clean check, it stops itself when the error count stops falling, \
and it reports which patterns actually moved the number on this repo.",
            Mode::ThinkerLiaison => "A large model with a small one relaying and condensing. The \
default: the liaison keeps the thinker's context clean and answers while it works.",
            Mode::LiaisonDebate => "Several small models debating, with the liaison summarizing. They \
can pause to ask YOU a question between messages, and one is held blind to your answer — so the next \
round shows whether your hint drove the conclusion or the question did.",
        }
    }
    pub fn all() -> [Mode; 8] {
        // Grouped by axis, and StatefulGuide sits LAST because it is the only member of a third
        // axis — the renderer prints a heading per axis change, so an interleaved list would print
        // "who reasons" twice. The contiguity test below is what enforces this.
        [Mode::Barebones, Mode::Coordinator, Mode::Aggressive, Mode::Replicator,
         Mode::LongHorizon, Mode::ThinkerLiaison, Mode::LiaisonDebate,
         Mode::StatefulGuide]
    }

    /// Which axis this mode varies.
    ///
    /// Six entries in one list, and they answer two different questions: Barebones/Coordinator/
    /// Aggressive vary HOW WIDE the fan-out is, and the other three vary WHO REASONS. An operator
    /// picking from an undifferentiated list of six has to already know that to choose sensibly.
    ///
    /// One list is still right — you pick one mode — but the list should say which question each entry
    /// answers rather than leaving it to be inferred from the names.
    pub fn axis(self) -> &'static str {
        match self {
            Mode::Barebones | Mode::Coordinator | Mode::Aggressive | Mode::Replicator => "how wide",
            Mode::LongHorizon | Mode::ThinkerLiaison | Mode::LiaisonDebate => "who reasons",
            // A THIRD question, and worth its own name rather than being filed under one of the
            // other two: StatefulGuide uses the same single large model as LongHorizon, so "who
            // reasons" is unchanged. What differs is who decides the run has progressed — the
            // checker instead of the narrative.
            Mode::StatefulGuide => "how it is steered",
        }
    }

    /// What actually happens when this mode is selected, today.
    ///
    /// The picker carried ONE blanket line — "Task decomposition + execution
    /// wiring is in progress; selecting a mode here records the intent" — and
    /// it was wrong in both directions. Some modes do considerably more than
    /// record an intent, and the thing that is genuinely missing is the same
    /// for all of them, so a per-mode sentence tells the operator what a shared
    /// disclaimer cannot: whether choosing this changes anything yet.
    ///
    /// Established by reading the call graph rather than the comments:
    /// [`plan`] builds a real [`Plan`] for every mode — system prompt, worker
    /// list, best-of, aggregation — and NOTHING CALLS IT. The planner is
    /// finished and unconsumed. What each mode already changes is whatever sits
    /// outside that unused path.
    pub fn wiring(self) -> &'static str {
        match self {
            Mode::Barebones =>
                "Live. One worker, no fan — this is what runs today whatever else is selected.",
            Mode::Replicator =>
                "Live. Selecting it opens the replicator UI: N columns over one workspace, one KV cache each, steerable mid-run.",
            Mode::Aggressive =>
                "Partly live. Switches the chat to the fan-out view, and plan() gives it best-of-3 per unit — but no runner consumes that plan yet, so the extra runs do not happen.",
            Mode::Coordinator =>
                "Planned, not run. plan() decomposes the goal into one worker prompt per subtask with shared context; nothing executes the result yet.",
            Mode::LongHorizon =>
                "Advisory only. Its one real effect today is that it cannot interrupt — can_interrupt() is false, so a run in this mode will not stop to ask.",
            Mode::ThinkerLiaison =>
                "Planned, not run. plan() emits a single worker; which model answers and who relays is the runtime's decision, and that runtime does not exist yet.",
            Mode::LiaisonDebate =>
                "Planned, not run. is_panel() is true and the panel prompts exist; nothing spawns the debaters.",
            Mode::StatefulGuide =>
                "Partly live. The phase machine in engmode is real and tested — survey → plan → patch → check → diagnose, with the phase decided by compiler output — but the loop that drives it from a chat turn is not wired.",
        }
    }

    /// Does selecting this mode change what happens on the next run?
    ///
    /// The picker greys nothing out — an operator should be able to record an
    /// intent — but a mode that changes nothing yet should say so where it is
    /// chosen rather than in a footnote under eight of them.
    pub fn has_effect(self) -> bool {
        matches!(self, Mode::Barebones | Mode::Replicator | Mode::Aggressive
                     | Mode::LongHorizon | Mode::StatefulGuide)
    }

    /// Does this mode run a debating panel?
    pub fn is_panel(self) -> bool { matches!(self, Mode::LiaisonDebate) }

    /// Can this mode stop and ask the operator mid-run?
    ///
    /// The long-horizon mode deliberately cannot: it exists for sessions where nobody is watching, and
    /// a question nobody answers stalls the run rather than improving it.
    pub fn can_interrupt(self) -> bool {
        !matches!(self, Mode::LongHorizon | Mode::Barebones)
            // Replicator CAN be interrupted — that is most of the point: you steer each column
            // mid-run, and a worker you cannot redirect is a batch job.
    
    }
}

/// A unit to parallelize — typically one function/item to write.
#[derive(Clone, Debug)]
pub struct Subtask { pub name: String, pub spec: String }

/// The job to plan. `subtasks` empty ⇒ nothing to fan out (every mode degrades to single-shot).
#[derive(Clone, Debug, Default)]
pub struct Task {
    pub goal: String,
    pub shared_context: String, // types / imports / the other file the workers must fit into
    pub subtasks: Vec<Subtask>,
}

#[derive(Clone, Debug)]
pub struct WorkerPrompt { pub label: String, pub system: String, pub user: String }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Aggregation { Single, StitchFiles }

#[derive(Clone, Debug)]
pub struct Plan {
    pub mode_name: &'static str,
    pub workers: Vec<WorkerPrompt>,
    pub best_of: usize,        // model runs per worker (fabric best-of-K); 1 for non-aggressive
    pub aggregate: Aggregation,
}

fn sys_barebones() -> String {
    "You are a precise coding assistant. Answer directly; lead with the code.".into()
}
/// The StatefulGuide base prompt. Deliberately short: it establishes the contract and the fact that
/// the phase is imposed from outside, and leaves the per-phase instruction to `engmode::prompt_for`
/// so the two cannot drift apart in two places.
fn sys_stateful() -> String {
    "You are a careful engineer working a long task in explicit PHASES. Each message you receive \
names the current phase and states what it requires; do that and only that. The phase is decided by \
the CHECKER (compiler and test output), not by you: you cannot advance by asserting progress, and a \
completion claim without a clean check will be rejected and shown back to you. Prefer the smallest \
edit that a compile can attribute. When a check fails, state the cause before proposing the fix — \
an edit without a stated cause is a resample, and resampling has already been tried."
        .into()
}

fn sys_worker() -> String {
    "You are one of several parallel workers writing ONE piece of a larger file. Implement ONLY your \
     assigned item as a complete, compilable definition — do not restate or redefine the sibling \
     items, and call them by the names given. No prose, no markdown fences unless the item needs \
     them.".into()
}
/// Aggressive mode runs the verify→feedback→revise loop, so its workers are told the work is checked
/// and may be iterated — which biases them toward code that actually compiles over clever guesses.
fn sys_worker_iterative() -> String {
    format!("{} This runs in an ITERATIVE loop: your output will be compiled (and may be tested) \
     automatically, and you may be given the resulting errors and asked to revise. Favor code that \
     compiles and handles the obvious edge cases over clever code that might not; prefer the standard \
     library unless a dependency is clearly required; if a detail is ambiguous, choose the simplest \
     correct option rather than guessing elaborately.", sys_worker())
}
fn sys_aggregate() -> String {
    "You are assembling parallel contributions into one coherent file. Resolve only the seams \
     (imports, ordering); do not rewrite the individual items.".into()
}

/// Build the orchestration plan for a mode + task. Pure: assembles prompts, picks fan-out + best-of +
/// aggregation. No model is called here.
pub fn plan(mode: Mode, task: &Task) -> Plan {
    let single = |sys: String| WorkerPrompt { label: "main".into(), system: sys, user: task.goal.clone() };
    match mode {
        // The reasoning modes plan like Barebones at THIS layer: one worker prompt. Who answers
        // it — a thinker, a thinker plus liaison, or a panel — is decided by the runtime that
        // consumes the plan. Folding it in here would conflate two axes and hand the panel a
        // fan-out width it never asked for.
        Mode::Barebones | Mode::LongHorizon | Mode::ThinkerLiaison | Mode::LiaisonDebate =>
            Plan { mode_name: "barebones", workers: vec![single(sys_barebones())], best_of: 1, aggregate: Aggregation::Single },
        // One worker, like the reasoning modes — the difference is entirely in the loop around it.
        // This layer supplies the PROTOCOL; the per-phase text comes from engmode::prompt_for at
        // run time, because the phase is not known until the checker has spoken.
        Mode::StatefulGuide =>
            Plan { mode_name: "stateful-guide", workers: vec![single(sys_stateful())], best_of: 1, aggregate: Aggregation::Single },
        // Replicator plans like Barebones ON PURPOSE. Its fan is not a decomposition of one goal —
        // it is N goals the operator types separately, one per column, steered live. There is
        // nothing for a planner to split here, and inventing a decomposition would put words in
        // workers the operator is about to instruct himself. The width lives in the mode's own
        // slider and the prompts arrive from the composer, not from this function.
        Mode::Replicator =>
            Plan { mode_name: "replicator", workers: vec![single(sys_barebones())], best_of: 1, aggregate: Aggregation::Single },
        Mode::Coordinator | Mode::Aggressive => {
            let best_of = if mode == Mode::Aggressive { 3 } else { 1 };
            let mode_name = if mode == Mode::Aggressive { "aggressive coordinator" } else { "coordinator" };
            if task.subtasks.is_empty() {
                // Nothing to fan out → a single worker (still honors best-of for aggressive).
                return Plan { mode_name, workers: vec![single(sys_barebones())], best_of, aggregate: Aggregation::Single };
            }
            let siblings = |exclude: &str| -> String {
                let s = task.subtasks.iter().filter(|s| s.name != exclude)
                    .map(|s| format!("- {}: {}", s.name, s.spec)).collect::<Vec<_>>().join("\n");
                if s.is_empty() { "(none)".into() } else { s }
            };
            let worker_sys = if mode == Mode::Aggressive { sys_worker_iterative() } else { sys_worker() };
            let workers = task.subtasks.iter().map(|st| {
                let user = format!(
                    "Overall goal:\n{goal}\n\n\
                     Shared context (types, imports, the other file you must fit into):\n{ctx}\n\n\
                     Your unit to implement: `{name}`\n\
                     Spec: {spec}\n\n\
                     Sibling units being written in parallel — match these names/signatures, call them, do NOT redefine them:\n{sib}\n\n\
                     Output ONLY `{name}` as a complete, compilable definition.",
                    goal = task.goal, ctx = task.shared_context, name = st.name, spec = st.spec, sib = siblings(&st.name));
                WorkerPrompt { label: st.name.clone(), system: worker_sys.clone(), user }
            }).collect();
            Plan { mode_name, workers, best_of, aggregate: Aggregation::StitchFiles }
        }
    }
}

/// The system prompt the aggregation/stitch step would use (when merge.rs leaves seams to a model).
pub fn aggregate_system() -> String { sys_aggregate() }

#[cfg(test)]
mod tests {
    #[test]
    fn all_modes_are_CONTIGUOUS_by_axis() {
        // The UI prints a heading whenever the axis changes, so an interleaved list would print the
        // same heading twice and read as a bug. Contiguity is a property of Mode::all() that the
        // renderer depends on and nothing else enforced.
        let axes: Vec<&str> = Mode::all().iter().map(|m| m.axis()).collect();
        let mut seen: Vec<&str> = Vec::new();
        for a in &axes {
            if seen.last() != Some(a) {
                assert!(!seen.contains(a), "axis {a:?} appears in two runs: {axes:?}");
                seen.push(a);
            }
        }
        assert_eq!(seen.len(), 3, "three axes, three headings: how wide / who reasons / how it is steered");
    }

    #[test]
    fn every_mode_has_an_axis_and_they_are_the_expected_two() {
        // The third axis was taken DELIBERATELY, which is what this guard asked for: StatefulGuide
        // runs the same single large model as LongHorizon, so filing it under "who reasons" would be
        // false. What varies is who decides the run advanced — the checker, not the narrative.
        for m in Mode::all() {
            assert!(matches!(m.axis(), "how wide" | "who reasons" | "how it is steered"),
                    "{m:?} has axis {:?}", m.axis());
        }
        assert_eq!(Mode::StatefulGuide.axis(), "how it is steered");
    }

    #[test]
    fn the_three_reasoning_modes_are_selectable() {
        // They were built (panelmode, panelvoice, deliberate) and never given an entry, which is why
        // the debate panel could not be found in the menu.
        let all = Mode::all();
        assert!(all.contains(&Mode::LongHorizon));
        assert!(all.contains(&Mode::ThinkerLiaison));
        assert!(all.contains(&Mode::LiaisonDebate));
        // Replicator repeated the exact failure this test was written for: the mode was built end
        // to end — slider, columns, dilation matrix, lane dispatch, persistence — and reachable
        // only by typing :replicate, so the operator went looking in this menu and correctly
        // concluded it did not exist. A mode you have to already know the name of is a mode nobody
        // finds.
        assert!(all.contains(&Mode::Replicator), "a built mode with no menu entry cannot be found");
        assert_eq!(all.len(), 8, "four fan-out widths, three reasoning shapes, one steering mode");
    }

    #[test]
    fn long_horizon_CANNOT_interrupt_and_that_is_the_point() {
        // It exists for sessions where nobody is watching. A question nobody answers stalls the run
        // rather than improving it, so the mode does not try — which is the actual difference from
        // thinker+liaison, not a limitation of it.
        assert!(!Mode::LongHorizon.can_interrupt());
        assert!(Mode::ThinkerLiaison.can_interrupt());
        assert!(Mode::LiaisonDebate.can_interrupt());
    }

    #[test]
    fn only_the_debate_mode_runs_a_panel() {
        assert!(Mode::LiaisonDebate.is_panel());
        for m in [Mode::Barebones, Mode::Coordinator, Mode::Aggressive,
                  Mode::LongHorizon, Mode::ThinkerLiaison] {
            assert!(!m.is_panel(), "{m:?}");
        }
    }

    #[test]
    fn every_mode_has_a_label_and_a_description() {
        // The dropdown shows both, and an empty one reads as a broken entry rather than a mode.
        for m in Mode::all() {
            assert!(!m.label().is_empty(), "{m:?}");
            assert!(m.blurb().len() > 20, "{m:?} needs a description someone can choose from");
        }
    }

    use super::*;

    fn task3() -> Task {
        Task {
            goal: "Implement a gesture classifier".into(),
            shared_context: "struct Path; enum Dir { N, S }".into(),
            subtasks: vec![
                Subtask { name: "classify".into(), spec: "Path -> Gesture".into() },
                Subtask { name: "thresholds".into(), spec: "tuning constants".into() },
                Subtask { name: "smooth".into(), spec: "denoise the path".into() },
            ],
        }
    }

    #[test]
    fn barebones_is_single_shot() {
        let p = plan(Mode::Barebones, &task3());
        assert_eq!(p.workers.len(), 1);
        assert_eq!(p.best_of, 1);
        assert_eq!(p.aggregate, Aggregation::Single);
        assert!(p.workers[0].user.contains("gesture classifier"), "carries the goal");
    }

    #[test]
    fn coordinator_fans_out_one_worker_per_subtask() {
        let p = plan(Mode::Coordinator, &task3());
        assert_eq!(p.workers.len(), 3, "one worker per subtask");
        assert_eq!(p.best_of, 1);
        assert_eq!(p.aggregate, Aggregation::StitchFiles);
    }

    #[test]
    fn each_worker_sees_its_spec_and_the_siblings_partial_fill() {
        let p = plan(Mode::Coordinator, &task3());
        let w = p.workers.iter().find(|w| w.label == "classify").unwrap();
        assert!(w.user.contains("`classify`"), "names its own unit");
        assert!(w.user.contains("Path -> Gesture"), "carries its own spec");
        // the partial-fill: it must see the sibling unit names but NOT be told to write them
        assert!(w.user.contains("thresholds") && w.user.contains("smooth"), "sees sibling units");
        assert!(!w.user.contains("- classify:"), "the sibling list excludes itself");
        assert!(w.user.contains("do NOT redefine"), "instructed to fit, not duplicate, siblings");
        // shared context threaded to every worker
        assert!(w.user.contains("enum Dir"), "carries the shared context");
    }

    #[test]
    fn aggressive_adds_best_of_k() {
        let p = plan(Mode::Aggressive, &task3());
        assert_eq!(p.workers.len(), 3);
        assert!(p.best_of >= 2, "aggressive runs best-of-K per unit, got {}", p.best_of);
        assert_eq!(p.aggregate, Aggregation::StitchFiles);
    }

    #[test]
    fn aggressive_workers_know_theyre_in_an_iterative_loop() {
        let agg = plan(Mode::Aggressive, &task3());
        let coord = plan(Mode::Coordinator, &task3());
        let asys = &agg.workers[0].system.to_lowercase();
        assert!(asys.contains("iterative") && asys.contains("compiled"), "aggressive worker is told about the verify/iterate loop");
        let csys = &coord.workers[0].system.to_lowercase();
        assert!(!csys.contains("iterative loop"), "plain coordinator worker isn't (no verify loop there)");
    }

    #[test]
    fn no_subtasks_degrades_to_single() {
        let t = Task { goal: "fix this bug".into(), ..Default::default() };
        for m in Mode::all() {
            let p = plan(m, &t);
            assert_eq!(p.workers.len(), 1, "{:?} with no subtasks is single-shot", m);
            assert_eq!(p.aggregate, Aggregation::Single);
        }
    }

    /// Every mode says what it does, and the claims match the code.
    #[test]
    fn every_mode_states_its_wiring() {
        for m in Mode::all() {
            let w = m.wiring();
            assert!(w.len() > 30, "{:?} has no real wiring note", m);
            assert!(w.ends_with('.'), "{:?}: {w}", m);
        }
    }

    /// `has_effect` must agree with the other predicates rather than drift from
    /// them — it is a summary of facts stated elsewhere in this file.
    #[test]
    fn has_effect_agrees_with_the_behaviour_it_summarises() {
        // LongHorizon's only current effect IS the interrupt rule.
        assert!(!Mode::LongHorizon.can_interrupt());
        assert!(Mode::LongHorizon.has_effect());

        // A mode whose note says "Planned, not run" must not claim an effect.
        for m in Mode::all() {
            if m.wiring().starts_with("Planned, not run") {
                assert!(!m.has_effect(), "{:?} says planned-not-run but claims an effect", m);
            }
        }
        // And one that claims an effect must not say it is unrun.
        for m in Mode::all() {
            if m.has_effect() {
                assert!(!m.wiring().starts_with("Planned, not run"),
                        "{:?} claims an effect and says it is not run", m);
            }
        }
    }

}
