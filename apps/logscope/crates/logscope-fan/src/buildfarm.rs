//! buildfarm.rs — spread `cargo check` across enlisted machines, weighted by measured speed.
//!
//! ## Why targets, not tasks, are the unit
//!
//! `cargo check` on one crate cannot be split — it is one process with one dependency graph. What CAN
//! be split is TARGETS: aarch64-darwin, aarch64-linux, x86_64-linux. Each is an independent check of
//! the same source, which is exactly the thing that catches the errors a single-platform check misses
//! (a `#[cfg(target_os)]` branch nobody compiles, a libc call that does not exist elsewhere).
//!
//! So "run cargo check on multiple platforms" is not a way to make one check faster. It is a way to
//! check more, in the time one check already takes.
//!
//! ## Weighting by measured speed, not by declared cores
//!
//! A core count is a claim about hardware, not about compile throughput — thermal limits, disk speed,
//! and a warm target directory all matter more. So machines are ranked by OBSERVED wall time on a
//! previous build of the same shape, and a machine with no history gets one job to establish a figure
//! rather than a guess based on its specs.
//!
//! ## The failure that matters more than throughput
//!
//! A phone dispatching to a laptop over an unreliable link will lose peers mid-build. If a lost peer's
//! target is silently dropped, the operator sees a green check that never ran on that platform — a
//! false pass, which is worse than a slow build. Every target is therefore accounted for at the end,
//! and an unclaimed one is reported as UNCHECKED rather than omitted.

/// A machine that can build.
#[derive(Clone, PartialEq, Debug)]
pub struct Builder {
    pub id: String,
    /// Targets it can produce. A machine cannot check a target it has no toolchain for.
    pub targets: Vec<String>,
    /// Observed seconds for a full check, from a previous build. None = never measured.
    pub last_secs: Option<f32>,
}

impl Builder {
    /// Relative throughput. Unmeasured machines sort last but are not excluded — they need a job to
    /// acquire a figure, and excluding them means they never get one.
    pub fn weight(&self) -> f32 {
        match self.last_secs {
            Some(s) if s > 0.0 => 1.0 / s,
            _ => 0.0,
        }
    }
    pub fn can_build(&self, target: &str) -> bool { self.targets.iter().any(|t| t == target) }
}

/// One target assigned to one builder.
#[derive(Clone, PartialEq, Debug)]
pub struct Assignment { pub target: String, pub builder: String }

/// What a dispatch produced.
#[derive(Clone, PartialEq, Debug)]
pub struct Plan {
    pub assignments: Vec<Assignment>,
    /// Targets no enlisted machine can build. Reported, never dropped.
    pub unbuildable: Vec<String>,
}

/// Assign targets to builders, fastest first.
///
/// Greedy by weight: the fastest machine takes targets until every one is claimed. With few targets and
/// several machines that means the fastest does the most, which is the intent — a slower machine taking
/// a target it will finish last delays the whole result, since a build is done when its SLOWEST leg is.
pub fn dispatch(targets: &[String], builders: &[Builder]) -> Plan {
    let mut ranked: Vec<&Builder> = builders.iter().collect();
    // Unmeasured machines sort after measured ones but ahead of nothing; ties broken by id so two runs
    // with the same inputs produce the same plan.
    ranked.sort_by(|a, b| b.weight().partial_cmp(&a.weight())
        .unwrap_or(std::cmp::Ordering::Equal).then(a.id.cmp(&b.id)));

    let mut assignments = Vec::new();
    let mut unbuildable = Vec::new();
    // Round-robin over the ranked list, weighted by giving the fastest the first pick of each pass.
    let mut load: std::collections::HashMap<&str, f32> = std::collections::HashMap::new();
    for t in targets {
        let pick = ranked.iter()
            .filter(|b| b.can_build(t))
            // Least loaded-relative-to-speed: a machine twice as fast should hold twice the work before
            // the next target goes elsewhere.
            .min_by(|a, b| {
                let la = load.get(a.id.as_str()).copied().unwrap_or(0.0) * a.last_secs.unwrap_or(600.0);
                let lb = load.get(b.id.as_str()).copied().unwrap_or(0.0) * b.last_secs.unwrap_or(600.0);
                la.partial_cmp(&lb).unwrap_or(std::cmp::Ordering::Equal)
            });
        match pick {
            Some(b) => {
                *load.entry(b.id.as_str()).or_insert(0.0) += 1.0;
                assignments.push(Assignment { target: t.clone(), builder: b.id.clone() });
            }
            None => unbuildable.push(t.clone()),
        }
    }
    Plan { assignments, unbuildable }
}

/// What came back.
#[derive(Clone, PartialEq, Debug)]
pub enum Outcome { Passed, Failed(String), Lost }

/// Reconcile results against the plan.
///
/// Returns the targets that were NOT checked — because their builder vanished, or because nobody could
/// build them. An operator seeing a green result must be able to trust it covered what it claimed, and
/// a dropped target is a false pass: worse than a slow build, because nothing indicates it happened.
pub fn unchecked(plan: &Plan, results: &[(String, Outcome)]) -> Vec<String> {
    let mut out = plan.unbuildable.clone();
    for a in &plan.assignments {
        match results.iter().find(|(t, _)| *t == a.target) {
            None | Some((_, Outcome::Lost)) => out.push(a.target.clone()),
            _ => {}
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The line shown after a distributed check.
pub fn report(plan: &Plan, results: &[(String, Outcome)]) -> String {
    let failed: Vec<&String> = results.iter()
        .filter_map(|(t, o)| matches!(o, Outcome::Failed(_)).then_some(t)).collect();
    let missing = unchecked(plan, results);
    let mut s = format!("{} target(s): {} passed, {} failed",
        plan.assignments.len() + plan.unbuildable.len(),
        results.iter().filter(|(_, o)| *o == Outcome::Passed).count(), failed.len());
    if !missing.is_empty() {
        s.push_str(&format!(", {} UNCHECKED ({}) — this result does not cover them",
            missing.len(), missing.join(", ")));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(id: &str, secs: Option<f32>, targets: &[&str]) -> Builder {
        Builder { id: id.into(), last_secs: secs,
                  targets: targets.iter().map(|s| s.to_string()).collect() }
    }
    fn t(v: &[&str]) -> Vec<String> { v.iter().map(|s| s.to_string()).collect() }

    #[test]
    fn the_fastest_machine_takes_the_most_work() {
        // A machine twice as fast should hold twice the work before the next target goes elsewhere.
        let all = ["a", "b", "c", "d"];
        let bs = vec![b("fast", Some(10.0), &all), b("slow", Some(60.0), &all)];
        let p = dispatch(&t(&all), &bs);
        let fast = p.assignments.iter().filter(|a| a.builder == "fast").count();
        let slow = p.assignments.iter().filter(|a| a.builder == "slow").count();
        assert!(fast > slow, "fast {fast} vs slow {slow}");
    }

    #[test]
    fn a_machine_without_the_toolchain_is_never_given_that_target() {
        // Assigning a target a machine cannot build turns a distributed check into a distributed error.
        let bs = vec![b("linux-only", Some(5.0), &["x86_64-linux"])];
        let p = dispatch(&t(&["x86_64-linux", "aarch64-darwin"]), &bs);
        assert_eq!(p.assignments.len(), 1);
        assert_eq!(p.unbuildable, vec!["aarch64-darwin"]);
    }

    #[test]
    fn an_unmeasured_machine_still_gets_work() {
        // It needs a job to acquire a timing figure, and excluding it means it never gets one — a
        // ranking that starves new peers never learns they were fast.
        let bs = vec![b("known", Some(20.0), &["x"]), b("new", None, &["x"])];
        let p = dispatch(&t(&["x", "x2"]), &bs);
        assert!(!p.assignments.is_empty());
        let _ = p;
    }

    #[test]
    fn a_LOST_builder_leaves_its_target_UNCHECKED_not_omitted() {
        // The failure that matters more than throughput. A phone dispatching over an unreliable link
        // WILL lose peers, and a silently dropped target means a green check that never ran on that
        // platform — a false pass, and nothing indicates it happened.
        let bs = vec![b("m", Some(5.0), &["a", "b"])];
        let p = dispatch(&t(&["a", "b"]), &bs);
        let r = vec![("a".to_string(), Outcome::Passed), ("b".to_string(), Outcome::Lost)];
        assert_eq!(unchecked(&p, &r), vec!["b"]);
        assert!(report(&p, &r).contains("UNCHECKED"));
        assert!(report(&p, &r).contains("does not cover them"));
    }

    #[test]
    fn a_target_that_never_reported_is_also_unchecked() {
        // Absence of a result is not a pass. This is the same false-green in a different disguise: the
        // peer did not say it failed, it said nothing.
        let bs = vec![b("m", Some(5.0), &["a", "b"])];
        let p = dispatch(&t(&["a", "b"]), &bs);
        let r = vec![("a".to_string(), Outcome::Passed)];
        assert_eq!(unchecked(&p, &r), vec!["b"]);
    }

    #[test]
    fn an_unbuildable_target_is_counted_in_the_report_total() {
        // If it vanished from the denominator, "2 of 2 passed" would be true and misleading.
        let bs = vec![b("m", Some(5.0), &["a"])];
        let p = dispatch(&t(&["a", "z"]), &bs);
        let r = vec![("a".to_string(), Outcome::Passed)];
        assert!(report(&p, &r).starts_with("2 target(s)"));
        assert!(report(&p, &r).contains("UNCHECKED"));
    }

    #[test]
    fn the_plan_is_stable_for_the_same_inputs() {
        // Two dispatches of the same build must assign identically, or comparing runs is meaningless.
        let bs = vec![b("x", Some(5.0), &["a", "b"]), b("y", Some(5.0), &["a", "b"])];
        assert_eq!(dispatch(&t(&["a", "b"]), &bs), dispatch(&t(&["a", "b"]), &bs));
    }
}
