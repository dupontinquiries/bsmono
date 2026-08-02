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
pub enum Mode { Barebones, Coordinator, Aggressive, LongHorizon, ThinkerLiaison, LiaisonDebate }

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Barebones => "Barebones",
            Mode::Coordinator => "Coordinator",
            Mode::Aggressive => "Aggressive coordinator",
            Mode::LongHorizon => "Long-horizon engineer",
            Mode::ThinkerLiaison => "Thinker + liaison",
            Mode::LiaisonDebate => "Liaison + debate panel",
        }
    }
    pub fn blurb(self) -> &'static str {
        match self {
            Mode::Barebones => "One model, one prompt — fastest, lowest load.",
            Mode::Coordinator => "Fans the task out to parallel workers (one per unit), each aware of the others, then stitches.",
            Mode::Aggressive => "Like Coordinator, plus best-of-K per unit — more parallel runs for higher quality.",
            // The three below vary WHO REASONS, where the three above vary how wide the fan-out is.
            // Two axes, deliberately in one list because an operator picks one thing.
            Mode::LongHorizon => "One large model, unguided. For runs where nobody is watching and the \
session has to carry itself — it cannot usefully stop and ask, so it does not try.",
            Mode::ThinkerLiaison => "A large model with a small one relaying and condensing. The \
default: the liaison keeps the thinker's context clean and answers while it works.",
            Mode::LiaisonDebate => "Several small models debating, with the liaison summarizing. They \
can pause to ask YOU a question between messages, and one is held blind to your answer — so the next \
round shows whether your hint drove the conclusion or the question did.",
        }
    }
    pub fn all() -> [Mode; 6] {
        [Mode::Barebones, Mode::Coordinator, Mode::Aggressive,
         Mode::LongHorizon, Mode::ThinkerLiaison, Mode::LiaisonDebate]
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
            Mode::Barebones | Mode::Coordinator | Mode::Aggressive => "how wide",
            Mode::LongHorizon | Mode::ThinkerLiaison | Mode::LiaisonDebate => "who reasons",
        }
    }

    /// Does this mode run a debating panel?
    pub fn is_panel(self) -> bool { matches!(self, Mode::LiaisonDebate) }

    /// Can this mode stop and ask the operator mid-run?
    ///
    /// The long-horizon mode deliberately cannot: it exists for sessions where nobody is watching, and
    /// a question nobody answers stalls the run rather than improving it.
    pub fn can_interrupt(self) -> bool {
        !matches!(self, Mode::LongHorizon | Mode::Barebones)
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
pub struct WorkerPrompt {
    pub label: String, pub system: String, pub user: String,
    /// Aggressive mode: one style per best-of run (len == best_of), so the K runs are K
    /// genuinely different worker threads — distinct instructions AND sampling — instead of
    /// K identical clones. Empty for other modes (uniform runs).
    pub styles: Vec<crate::styles::Style>,
}

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
    let single = |sys: String| WorkerPrompt { label: "main".into(), system: sys, user: task.goal.clone(), styles: Vec::new() };
    match mode {
        // The reasoning modes plan like Barebones at THIS layer: one worker prompt. Who answers
        // it — a thinker, a thinker plus liaison, or a panel — is decided by the runtime that
        // consumes the plan. Folding it in here would conflate two axes and hand the panel a
        // fan-out width it never asked for.
        Mode::Barebones | Mode::LongHorizon | Mode::ThinkerLiaison | Mode::LiaisonDebate =>
            Plan { mode_name: "barebones", workers: vec![single(sys_barebones())], best_of: 1, aggregate: Aggregation::Single },
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
            let workers = task.subtasks.iter().enumerate().map(|(ui, st)| {
                let user = format!(
                    "Overall goal:\n{goal}\n\n\
                     Shared context (types, imports, the other file you must fit into):\n{ctx}\n\n\
                     Your unit to implement: `{name}`\n\
                     Spec: {spec}\n\n\
                     Sibling units being written in parallel — match these names/signatures, call them, do NOT redefine them:\n{sib}\n\n\
                     Output ONLY `{name}` as a complete, compilable definition.",
                    goal = task.goal, ctx = task.shared_context, name = st.name, spec = st.spec, sib = siblings(&st.name));
                WorkerPrompt { styles: if mode == Mode::Aggressive { crate::styles::assign(best_of, ui as u64) } else { Vec::new() }, label: st.name.clone(), system: worker_sys.clone(), user }
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
        assert_eq!(seen.len(), 2, "two axes, two headings");
    }

    #[test]
    fn every_mode_has_an_axis_and_they_are_the_expected_two() {
        // A third axis is not wrong, but it would need a UI decision rather than appearing silently.
        for m in Mode::all() {
            assert!(matches!(m.axis(), "how wide" | "who reasons"), "{m:?} has axis {:?}", m.axis());
        }
    }

    #[test]
    fn the_three_reasoning_modes_are_selectable() {
        // They were built (panelmode, panelvoice, deliberate) and never given an entry, which is why
        // the debate panel could not be found in the menu.
        let all = Mode::all();
        assert!(all.contains(&Mode::LongHorizon));
        assert!(all.contains(&Mode::ThinkerLiaison));
        assert!(all.contains(&Mode::LiaisonDebate));
        assert_eq!(all.len(), 6, "three fan-out widths plus three reasoning shapes");
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
}

#[cfg(test)]
mod styled_tests {
    use super::*;
    #[test]
    fn aggressive_workers_carry_k_distinct_styles() {
        let t = Task { goal: "g".into(), shared_context: "ctx".into(),
                       subtasks: vec![Subtask { name: "a".into(), spec: "s".into() },
                                      Subtask { name: "b".into(), spec: "s".into() }] };
        let p = plan(Mode::Aggressive, &t);
        assert_eq!(p.best_of, 3);
        for w in &p.workers {
            assert_eq!(w.styles.len(), 3, "one style per best-of run");
            let names: std::collections::BTreeSet<_> = w.styles.iter().map(|s| s.name).collect();
            assert_eq!(names.len(), 3, "styles distinct within a unit");
        }
        // different units rotate differently (seeded by unit index)
        assert_ne!(p.workers[0].styles[0].name, p.workers[1].styles[0].name);
        let pc = plan(Mode::Coordinator, &t);
        assert!(pc.workers.iter().all(|w| w.styles.is_empty()), "non-aggressive is uniform");
    }
}
