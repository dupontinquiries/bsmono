//! longterm.rs — the `:longterm` planner.
//!
//! Turns one objective paragraph into a bounded, long-horizon orchestration *plan* over logscope's
//! existing primitives rather than new machinery: [`crate::coordinator`] (decompose → sibling-aware
//! workers), [`crate::directives`] (behavioural fan-out per lane), and a budget. The command *plans* a
//! use of the primitives; the model-driven decomposition and the worker execution are driven by the
//! fabric/runner on the device — this module is the pure planner that sits in front of them and is
//! fully unit-tested.
//!
//! Usage (the `:longterm` prefix is stripped by the dispatcher; this parses the argument):
//!
//! ```text
//! :longterm make sure to unify the code duplications across the repo, prioritising the ones with
//!           three or more instances, and produce a migration plan I can execute one commit at a time
//! :longterm --deep  <objective>     # more lanes, more directives, deeper expansion, bigger budget
//! :longterm --quick <objective>     # one decomposition pass, one directive per lane, no expansion
//! ```

use crate::coordinator::{Mode, Subtask, Task};
use crate::directives::{self, Directive};

/// How ambitious the run is — mirrors the coordinator's barebones / coordinator / aggressive split.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Depth {
    Quick,
    Balanced,
    Deep,
}

impl Depth {
    /// `(target subgoals, directives per subgoal, expansion depth)` — the shape of the fan-out tree.
    pub fn shape(self) -> (usize, usize, usize) {
        match self {
            Depth::Quick => (3, 1, 1),    // one pass, one directive each, no expansion
            Depth::Balanced => (4, 2, 2), // fan-out + one level of expansion of the best lanes
            Depth::Deep => (6, 3, 3),     // more lanes, more directives, deeper
        }
    }
    /// Which coordinator mode executes each lane's fan-out.
    pub fn coordinator_mode(self) -> Mode {
        match self {
            Depth::Quick => Mode::Barebones,
            Depth::Balanced => Mode::Coordinator,
            Depth::Deep => Mode::Aggressive,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Depth::Quick => "quick",
            Depth::Balanced => "balanced",
            Depth::Deep => "deep",
        }
    }
}

/// The bounds a run stays inside; any exceeded ⇒ stop and report what's done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    pub tokens: u64,
    pub wall_secs: u64,
    pub max_depth: usize,
}

/// A parsed `:longterm` invocation — the plan parameters, before the (model-driven) decomposition.
#[derive(Clone, Debug, PartialEq)]
pub struct Longterm {
    pub objective: String,
    pub depth: Depth,
    pub subgoals_target: usize,
    pub directives_per: usize,
    pub budget: Budget,
    /// Deterministic seed so a plan (and its directive fan-out) is reproducible.
    pub seed: u64,
}

/// Parse the argument text of `:longterm` (flags may appear anywhere; the rest is the objective).
pub fn parse(arg: &str, seed: u64) -> Longterm {
    let mut depth = Depth::Balanced;
    let mut words = Vec::new();
    for tok in arg.split_whitespace() {
        match tok {
            "--deep" | "-d" => depth = Depth::Deep,
            "--quick" | "-q" => depth = Depth::Quick,
            "--balanced" | "-b" => depth = Depth::Balanced,
            _ => words.push(tok),
        }
    }
    let (subgoals_target, directives_per, max_depth) = depth.shape();
    let budget = match depth {
        Depth::Quick => Budget { tokens: 200_000, wall_secs: 600, max_depth },
        Depth::Balanced => Budget { tokens: 1_000_000, wall_secs: 3_600, max_depth },
        Depth::Deep => Budget { tokens: 4_000_000, wall_secs: 14_400, max_depth },
    };
    Longterm { objective: words.join(" "), depth, subgoals_target, directives_per, budget, seed }
}

impl Longterm {
    pub fn mode(&self) -> Mode {
        self.depth.coordinator_mode()
    }

    /// Assign a deterministic set of directives to each of `n` subgoals (the model-produced count),
    /// reusing the directive library's selection, offset per lane so lanes explore differently. This is
    /// the behavioural fan-out the tree spreads over.
    pub fn directive_lanes(&self, n: usize) -> Vec<Vec<&'static Directive>> {
        (0..n)
            .map(|i| directives::select(self.directives_per, self.seed.wrapping_add(i as u64)))
            .collect()
    }

    /// Build the coordinator [`Task`] from the objective and the model-produced subgoals — each subgoal
    /// becomes a sibling-aware lane. [`crate::coordinator::plan`] turns this into the worker prompts;
    /// the fabric executes them. Returns a Task with no subtasks when `subgoals` is empty (degrades to a
    /// single-shot pass, exactly like the other modes).
    pub fn to_task(&self, subgoals: &[(String, String)]) -> Task {
        Task {
            goal: self.objective.clone(),
            shared_context: format!(
                "Long-horizon objective under a {} plan (budget ~{}k tokens / {}s, depth {}). Record \
                 findings to the blackboard so sibling lanes build on them, not blind.",
                self.depth.label(),
                self.budget.tokens / 1000,
                self.budget.wall_secs,
                self.budget.max_depth,
            ),
            subtasks: subgoals
                .iter()
                .map(|(name, spec)| Subtask { name: name.clone(), spec: spec.clone() })
                .collect(),
        }
    }

    /// A human-readable summary of the plan (the header of the plan artifact the run produces).
    pub fn summary(&self) -> String {
        format!(
            "objective: {}\nplan: {} — {} subgoals × {} directive(s), expand to depth {}\nbudget: ~{}k tokens, {}s, depth {}",
            self.objective,
            self.depth.label(),
            self.subgoals_target,
            self.directives_per,
            self.budget.max_depth,
            self.budget.tokens / 1000,
            self.budget.wall_secs,
            self.budget.max_depth,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_extracts_objective_and_defaults_to_balanced() {
        let lt = parse("unify the code duplications across the repo", 7);
        assert_eq!(lt.depth, Depth::Balanced);
        assert_eq!(lt.objective, "unify the code duplications across the repo");
        assert_eq!((lt.subgoals_target, lt.directives_per), (4, 2));
        assert_eq!(lt.mode(), Mode::Coordinator);
    }

    #[test]
    fn flags_select_depth_and_are_stripped_from_the_objective() {
        let deep = parse("--deep rework the build system", 0);
        assert_eq!(deep.depth, Depth::Deep);
        assert_eq!(deep.objective, "rework the build system"); // flag removed
        assert_eq!(deep.mode(), Mode::Aggressive);

        let quick = parse("draft a plan --quick", 0);
        assert_eq!(quick.depth, Depth::Quick);
        assert_eq!(quick.objective, "draft a plan");
        assert_eq!(quick.budget.tokens, 200_000);
        assert!(deep.budget.tokens > quick.budget.tokens);
    }

    #[test]
    fn directive_lanes_are_deterministic_and_sized() {
        let lt = parse("--deep x", 42); // deep ⇒ 3 directives per lane
        let a = lt.directive_lanes(5);
        assert_eq!(a.len(), 5);
        assert!(a.iter().all(|lane| lane.len() == 3));
        // reproducible for the same seed
        let b = parse("--deep x", 42).directive_lanes(5);
        let names = |v: &Vec<Vec<&Directive>>| v.iter().map(|l| l.iter().map(|d| d.name).collect::<Vec<_>>()).collect::<Vec<_>>();
        assert_eq!(names(&a), names(&b));
    }

    #[test]
    fn to_task_maps_subgoals_to_sibling_lanes() {
        let lt = parse("unify duplication", 1);
        let task = lt.to_task(&[
            ("find-clusters".into(), "find duplication clusters of 3+ instances".into()),
            ("migration-plan".into(), "draft a one-commit-at-a-time migration".into()),
        ]);
        assert_eq!(task.goal, "unify duplication");
        assert_eq!(task.subtasks.len(), 2);
        assert_eq!(task.subtasks[0].name, "find-clusters");
        assert!(task.shared_context.contains("blackboard")); // lanes share findings
        // empty subgoals ⇒ single-shot degrade
        assert!(lt.to_task(&[]).subtasks.is_empty());
    }
}
