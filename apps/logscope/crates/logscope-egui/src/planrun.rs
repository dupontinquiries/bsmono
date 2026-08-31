//! planrun — the missing link between the planner and the runner.
//!
//! ## What was actually missing
//!
//! `coordinator::plan()` builds a complete worker plan for every mode — system prompt, one worker
//! per subtask with sibling context, best-of, aggregation — and nothing has ever consumed it. The
//! mode picker says so under most of the modes.
//!
//! It is easy to read that as "someone needs to write a runner". It is not: `AgentFan` already runs
//! N branches over isolated workspaces and folds them. The real gap is one field. `plan()` fans out
//! over `Task.subtasks`, and NOTHING IN THE TREE PRODUCES SUBTASKS — `decompose` is a different
//! thing (skeleton holes with `todo!()` bodies), and every caller would have had to invent them.
//! With `subtasks` empty, `plan()` degrades to a single worker, which is exactly what the loop does
//! already. So the planner was not unconsumed because it was hard to run; it was unconsumed because
//! it was always being handed an empty task.
//!
//! ## Where subtasks come from
//!
//! From the plan the model already writes. Turn one of every agent run emits a ```plan block, the
//! loop already parses it, and since plan-checking each step carries the files it names. Those steps
//! ARE the decomposition — stated by the model, visible to the operator, and (this is the part worth
//! having) checkable afterwards by the same rule that checks a single run's plan.
//!
//! That closes the loop the request asked for: one plan, used to fan the work out AND to decide
//! afterwards whether it was done. A decomposition invented separately from the plan the operator
//! read would be neither.
//!
//! ## What this module does not do
//!
//! It does not run anything and does not talk to a model — it maps plan steps to a
//! `coordinator::Task` and worker prompts to branch goals. The running is `AgentFan`'s, the planning
//! is `coordinator`'s, and keeping this layer pure is what lets the mapping be tested without a GPU.

use logscope_fan::coordinator::{self, Mode, Subtask, Task, WorkerPrompt};

/// Turn parsed plan steps into a coordinator `Task`.
///
/// Naming: a subtask is named for the file it targets when it names one, because that name appears
/// in every sibling's prompt ("match these names, do NOT redefine them") and a path is the one
/// identifier two workers cannot disagree about. Steps naming no file fall back to `step-N` —
/// they still get a worker, they just cannot be referred to by target.
pub fn task_from_steps(goal: &str, steps: &[String], shared_context: &str) -> Task {
    let mut subtasks = Vec::new();
    for (i, text) in steps.iter().enumerate() {
        let t = text.trim();
        if t.is_empty() { continue; }
        let name = crate::plancheck::step_targets(t).into_iter().next()
            .unwrap_or_else(|| format!("step-{}", i + 1));
        // A duplicate name would make two workers' sibling lists ambiguous about which is which.
        let name = if subtasks.iter().any(|s: &Subtask| s.name == name) {
            format!("{name}#{}", i + 1)
        } else { name };
        subtasks.push(Subtask { name, spec: t.to_string() });
    }
    Task { goal: goal.to_string(), shared_context: shared_context.to_string(), subtasks }
}

/// One branch to run: the label the fan shows, the system prompt (the worker's stance from the
/// planner) and the user prompt (its unit, with sibling context).
#[derive(Clone, Debug, PartialEq)]
pub struct Branch {
    pub label: String,
    pub system: String,
    pub goal: String,
}

/// Plan the work, then hand back branches a runner can execute.
///
/// `max_branches` bounds the fan: a fifteen-step plan is a legitimate plan and fifteen concurrent
/// workspaces is not. Truncation is reported by the caller rather than hidden — the same rule as the
/// check-root cap, learned the same way.
pub fn branches(mode: Mode, task: &Task, max_branches: usize) -> (Vec<Branch>, usize) {
    let plan = coordinator::plan(mode, task);
    let total = plan.workers.len();
    let kept: Vec<Branch> = plan.workers.into_iter().take(max_branches.max(1))
        .map(|w: WorkerPrompt| Branch { label: w.label, system: w.system, goal: w.user })
        .collect();
    let dropped = total.saturating_sub(kept.len());
    (kept, dropped)
}

/// How many model runs the plan implies, counting best-of. The number an operator should see BEFORE
/// a fan starts: `best_of` is invisible in the branch list and silently multiplies the cost.
pub fn run_count(mode: Mode, task: &Task, max_branches: usize) -> usize {
    let plan = coordinator::plan(mode, task);
    plan.workers.len().min(max_branches.max(1)) * plan.best_of.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps(v: &[&str]) -> Vec<String> { v.iter().map(|s| s.to_string()).collect() }

    #[test]
    fn plan_steps_become_subtasks_named_for_what_they_touch() {
        let t = task_from_steps("make it build",
            &steps(&["rewrite src/a.rs", "rewrite src/b.rs", "work out why it hangs"]), "shared types");
        assert_eq!(t.subtasks.len(), 3);
        assert_eq!(t.subtasks[0].name, "src/a.rs");
        assert_eq!(t.subtasks[1].name, "src/b.rs");
        assert_eq!(t.subtasks[2].name, "step-3", "a step naming no file still gets a worker");
        assert_eq!(t.goal, "make it build");
        assert_eq!(t.shared_context, "shared types");
    }

    #[test]
    fn two_steps_touching_one_file_do_not_collide() {
        let t = task_from_steps("g", &steps(&["edit src/a.rs", "also edit src/a.rs"]), "");
        assert_ne!(t.subtasks[0].name, t.subtasks[1].name,
                   "sibling lists must be able to tell two workers apart");
        assert!(t.subtasks[1].name.starts_with("src/a.rs"));
    }

    #[test]
    fn empty_steps_are_dropped_rather_than_becoming_empty_workers() {
        let t = task_from_steps("g", &steps(&["do a thing", "   ", ""]), "");
        assert_eq!(t.subtasks.len(), 1);
    }

    /// The whole point: with subtasks present, the planner fans out. With none, it degrades to one
    /// worker — which is what every caller was getting before, and why the planner looked unused.
    #[test]
    fn the_planner_fans_out_once_it_is_given_subtasks() {
        let with = task_from_steps("g", &steps(&["edit src/a.rs", "edit src/b.rs", "edit src/c.rs"]), "ctx");
        let (bs, dropped) = branches(Mode::Coordinator, &with, 8);
        assert_eq!(bs.len(), 3);
        assert_eq!(dropped, 0);
        assert_eq!(bs[0].label, "src/a.rs");
        // each worker is told about its siblings and told not to redefine them
        assert!(bs[0].goal.contains("src/b.rs") && bs[0].goal.contains("do NOT redefine"), "{}", bs[0].goal);
        assert!(bs[0].goal.contains("ctx"), "shared context reaches the worker");
        assert!(!bs[0].system.is_empty());

        let without = task_from_steps("g", &[], "");
        let (one, _) = branches(Mode::Coordinator, &without, 8);
        assert_eq!(one.len(), 1, "no subtasks → the single worker every caller was already getting");
    }

    #[test]
    fn the_branch_cap_reports_what_it_dropped() {
        let t = task_from_steps("g", &steps(&["edit a.rs", "edit b.rs", "edit c.rs", "edit d.rs"]), "");
        let (bs, dropped) = branches(Mode::Coordinator, &t, 2);
        assert_eq!(bs.len(), 2);
        assert_eq!(dropped, 2, "a hidden truncation is the bug this avoids");
    }

    #[test]
    fn best_of_is_visible_in_the_run_count_before_anything_starts() {
        let t = task_from_steps("g", &steps(&["edit a.rs", "edit b.rs"]), "");
        assert_eq!(run_count(Mode::Coordinator, &t, 8), 2);
        // Aggressive is best-of-3 per unit: six runs, not two. Invisible in the branch list.
        assert_eq!(run_count(Mode::Aggressive, &t, 8), 6);
        assert_eq!(run_count(Mode::Barebones, &t, 8), 1);
    }

    #[test]
    fn a_single_worker_mode_ignores_the_decomposition() {
        let t = task_from_steps("g", &steps(&["edit a.rs", "edit b.rs"]), "");
        let (bs, _) = branches(Mode::StatefulGuide, &t, 8);
        assert_eq!(bs.len(), 1, "the phase machine drives one run, not a fan");
        assert_eq!(bs[0].label, "main");
    }
}
