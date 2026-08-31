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

use logscope_fan::coordinator::{self, Mode, Subtask, Task};

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

/// Plan the work, then hand back the branches a runner should execute — INCLUDING best-of copies.
///
/// `best_of` is expanded here rather than left to the runner, and that is a correction: an earlier
/// version returned one branch per worker and separately reported `run_count` as
/// `workers * best_of`. Nothing in `AgentFan` has ever implemented best-of, so the number shown to
/// the operator ("12 model runs") described work that would never happen. Producing the copies here
/// makes the reported count and the executed count the same list — they cannot drift, because
/// there is only one of them.
///
/// `max_runs` is a budget on TOTAL runs, not on workers, since a run is what costs time and a
/// workspace. Aggressive at best-of-3 with a budget of 6 therefore plans two units properly rather
/// than six units badly. Truncation is returned, never hidden.
pub fn branches(mode: Mode, task: &Task, max_runs: usize) -> (Vec<Branch>, usize) {
    let plan = coordinator::plan(mode, task);
    let best_of = plan.best_of.max(1);
    let budget = max_runs.max(1);
    // How many distinct units fit once each unit costs `best_of` runs.
    let units = (budget / best_of).max(1).min(plan.workers.len());
    let dropped_units = plan.workers.len().saturating_sub(units);
    let mut out = Vec::new();
    for w in plan.workers.into_iter().take(units) {
        for i in 0..best_of {
            // Distinct labels: the fan's review picks between attempts at the SAME unit, and two
            // branches sharing a name make the verdict unreadable.
            let label = if best_of == 1 { w.label.clone() } else { format!("{} #{}", w.label, i + 1) };
            out.push(Branch { label, system: w.system.clone(), goal: w.user.clone() });
        }
    }
    (out, dropped_units)
}

/// How many model runs the plan implies. Equal to `branches(..).0.len()` BY CONSTRUCTION — it calls
/// it — so the figure an operator is shown before a fan starts is the figure that will run.
pub fn run_count(mode: Mode, task: &Task, max_runs: usize) -> usize {
    branches(mode, task, max_runs).0.len()
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
    fn the_branch_cap_budgets_total_runs_and_reports_dropped_units() {
        let t = task_from_steps("g", &steps(&["edit a.rs", "edit b.rs", "edit c.rs", "edit d.rs"]), "");
        let (bs, dropped) = branches(Mode::Coordinator, &t, 2);
        assert_eq!(bs.len(), 2);
        assert_eq!(dropped, 2, "a hidden truncation is the bug this avoids");
        // Aggressive costs 3 runs per unit, so the same budget buys fewer UNITS — two units done
        // properly beats six units the budget cannot pay for.
        let (bs3, dropped3) = branches(Mode::Aggressive, &t, 6);
        assert_eq!(bs3.len(), 6, "6 runs = 2 units x best-of-3");
        assert_eq!(dropped3, 2);
    }

    /// The correction this module needed: the count shown to the operator and the list actually run
    /// must be the same thing. They were not — best_of was reported and never executed.
    #[test]
    fn best_of_produces_the_runs_it_claims() {
        let t = task_from_steps("g", &steps(&["edit a.rs", "edit b.rs"]), "");
        for mode in [Mode::Coordinator, Mode::Aggressive, Mode::Barebones] {
            let (bs, _) = branches(mode, &t, 12);
            assert_eq!(bs.len(), run_count(mode, &t, 12),
                       "{mode:?}: the reported run count is not the list that runs");
        }
        // Aggressive really does produce three attempts at each unit, distinctly labelled.
        let (bs, _) = branches(Mode::Aggressive, &t, 12);
        assert_eq!(bs.len(), 6);
        let labels: Vec<&str> = bs.iter().map(|b| b.label.as_str()).collect();
        assert_eq!(labels, ["a.rs #1", "a.rs #2", "a.rs #3", "b.rs #1", "b.rs #2", "b.rs #3"]);
        // attempts at one unit are the same job, so they share a goal
        assert_eq!(bs[0].goal, bs[1].goal);
        assert_ne!(bs[0].goal, bs[3].goal);
        // and a best-of-1 mode is not given noise suffixes
        let (one, _) = branches(Mode::Coordinator, &t, 12);
        assert_eq!(one[0].label, "a.rs");
    }

    #[test]
    fn a_single_worker_mode_ignores_the_decomposition() {
        let t = task_from_steps("g", &steps(&["edit a.rs", "edit b.rs"]), "");
        let (bs, _) = branches(Mode::StatefulGuide, &t, 8);
        assert_eq!(bs.len(), 1, "the phase machine drives one run, not a fan");
        assert_eq!(bs[0].label, "main");
    }
}
