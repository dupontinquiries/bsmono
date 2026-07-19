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
pub enum Mode { Barebones, Coordinator, Aggressive }

impl Mode {
    pub fn label(self) -> &'static str {
        match self { Mode::Barebones => "Barebones", Mode::Coordinator => "Coordinator", Mode::Aggressive => "Aggressive coordinator" }
    }
    pub fn blurb(self) -> &'static str {
        match self {
            Mode::Barebones => "One model, one prompt — fastest, lowest load.",
            Mode::Coordinator => "Fans the task out to parallel workers (one per unit), each aware of the others, then stitches.",
            Mode::Aggressive => "Like Coordinator, plus best-of-K per unit — more parallel runs for higher quality.",
        }
    }
    pub fn all() -> [Mode; 3] { [Mode::Barebones, Mode::Coordinator, Mode::Aggressive] }
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
        Mode::Barebones => Plan { mode_name: "barebones", workers: vec![single(sys_barebones())], best_of: 1, aggregate: Aggregation::Single },
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
