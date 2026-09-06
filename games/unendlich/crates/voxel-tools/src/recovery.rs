//! Recovery: what happens when part of a build fails.
//!
//! Four things go wrong in practice, and until now each was handled by
//! the least useful policy available:
//!
//! 1. **One line of a plan will not parse.** The whole plan was
//!    rejected. A 1B that gets eight lines right and one wrong got
//!    nothing built and a wall of errors.
//! 2. **An op fails mid-plan.** The run continued, which is right when
//!    the failed op was decorative and wrong when it was structural: a
//!    wall refused for size leaves the roof that came after it floating
//!    on nothing.
//! 3. **Blocks are skipped at flush** because their chunk is not loaded.
//!    Said once in the chat, then forgotten. The build has a hole that
//!    nothing will ever fill.
//! 4. **The world is lost or rolled back** and the builds with it,
//!    though the conversation that made them survived in the workset.
//!
//! The policies here, one per failure:
//!
//! 1. SALVAGE. Run the lines that parse; report the ones that do not
//!    with enough detail for a repair turn; keep the errors so the
//!    model can be asked to fix only them.
//! 2. ROLLBACK DEPENDENTS. Ops declare whether they stand on their own.
//!    When a structural op fails, everything after it in the same plan
//!    that depends on structure is undone, and the player is told what
//!    was withdrawn and why. Decorative failures are just skipped.
//! 3. DEFER. Skipped cells are remembered with their target block and
//!    retried when their chunk appears. The player can see how many are
//!    waiting and can drop them.
//! 4. REPLAY. A workset's journal is its build sequence. Replaying it
//!    against a fresh world reconstructs the build.

use crate::plan::{Op, Severity};
use crate::weather::Change;

/// Does an op make sense without the structure before it?
///
/// A roof on nothing, a floor in the air, stairs to nowhere, a mirror
/// of an absent thing: these depend on what came earlier in the plan.
/// A tree, a path or a terraform stand alone. This is what decides how
/// far a failure propagates.
pub fn depends_on_structure(op: &Op) -> bool {
    matches!(op,
        Op::Roof { .. } | Op::Floor { .. } | Op::Stairs { .. } | Op::Mirror { .. }
        | Op::Restyle { .. } | Op::Detail { .. } | Op::Weather { .. } | Op::Mood { .. }
        | Op::Populate { .. })
}

/// Is this op the kind other ops stand on?
pub fn is_structural(op: &Op) -> bool {
    matches!(op,
        Op::Fill { .. } | Op::Cylinder { .. } | Op::Enclose { .. } | Op::Sphere { .. }
        | Op::Form { .. } | Op::Views { .. } | Op::Place { .. } | Op::Bridge { .. }
        | Op::Road { .. })
}

/// What was withdrawn after a structural failure, for the chat log.
#[derive(Debug, Clone, PartialEq)]
pub struct Withdrawal {
    /// 1-based index of the op that failed.
    pub failed_op: usize,
    pub why: String,
    /// 1-based indices of dependents that were rolled back.
    pub rolled_back: Vec<usize>,
}

impl Withdrawal {
    pub fn message(&self) -> String {
        if self.rolled_back.is_empty() {
            format!("op {} failed ({}); nothing depended on it", self.failed_op, self.why)
        } else {
            format!("op {} failed ({}); withdrew op(s) {} which stood on it",
                    self.failed_op, self.why,
                    self.rolled_back.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", "))
        }
    }
}

/// Given the per-op outcomes of a run, decide which dependents to
/// withdraw. Pure: returns indices; the caller undoes their edits.
///
/// The rule: after the FIRST structural failure, every later op that
/// depends on structure is withdrawn. Later structural ops are kept
/// (they may stand on earlier successes), and later independent ops
/// are kept (a tree does not care).
pub fn plan_rollback(ops: &[Op], ok: &[bool], reasons: &[String]) -> Vec<Withdrawal> {
    let mut out = Vec::new();
    let mut first_structural_failure: Option<usize> = None;
    for (i, op) in ops.iter().enumerate() {
        if !ok[i] && is_structural(op) && first_structural_failure.is_none() {
            first_structural_failure = Some(i);
            let rolled: Vec<usize> = ops.iter().enumerate().skip(i + 1)
                .filter(|(j, o)| ok[*j] && depends_on_structure(o))
                .map(|(j, _)| j + 1)
                .collect();
            out.push(Withdrawal {
                failed_op: i + 1,
                why: reasons.get(i).cloned().unwrap_or_default(),
                rolled_back: rolled,
            });
        }
    }
    out
}

/// Lines a model got wrong, kept so it can be asked to fix ONLY those.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Salvage {
    /// IR that parsed and can run.
    pub good: String,
    /// (line number, the line, why it failed).
    pub bad: Vec<(usize, String, String)>,
}

impl Salvage {
    pub fn is_total_loss(&self) -> bool { self.good.trim().is_empty() }

    /// The follow-up turn: the bad lines and their errors, nothing
    /// else. A model shown its whole plan again rewrites the parts that
    /// were fine; shown only the failures, it fixes them.
    pub fn repair_prompt(&self) -> String {
        let mut s = String::from(
            "These lines from your plan could not be used. Rewrite ONLY these lines, \
             fixed, one per line, nothing else:\n");
        for (n, line, why) in &self.bad {
            s.push_str(&format!("  line {n}: {line}\n    problem: {why}\n"));
        }
        s
    }

    /// Merge a repair reply back: the good lines plus the fixed ones.
    pub fn merge(&self, fixed: &str) -> String {
        let mut s = self.good.clone();
        if !s.ends_with('\n') && !s.is_empty() { s.push('\n'); }
        s.push_str(fixed.trim());
        s.push('\n');
        s
    }
}

/// Split a plan's parse result into salvageable and not.
///
/// Errors are keyed to line numbers in their message ("line 7: ..."),
/// which is how the bad lines are identified without re-parsing.
pub fn salvage(ir: &str, issues: &[crate::plan::Issue]) -> Salvage {
    let mut bad_lines: std::collections::BTreeMap<usize, String> = Default::default();
    for i in issues.iter().filter(|i| i.severity == Severity::Error) {
        if let Some(n) = line_number_in(&i.message) {
            bad_lines.entry(n).or_insert_with(|| i.message.clone());
        }
    }
    let mut good = String::new();
    let mut bad = Vec::new();
    for (idx, line) in ir.lines().enumerate() {
        let n = idx + 1;
        match bad_lines.get(&n) {
            Some(why) => bad.push((n, line.to_string(), why.clone())),
            None => { good.push_str(line); good.push('\n'); }
        }
    }
    Salvage { good, bad }
}

fn line_number_in(msg: &str) -> Option<usize> {
    let i = msg.find("line ")?;
    msg[i + 5..].split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

/// Cells that could not be written because their chunk was not loaded,
/// kept for a retry.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Deferred {
    pub cells: Vec<Change>,
    /// Which workset they belong to, if any, so a replay can attribute.
    pub workset: Option<String>,
}

impl Deferred {
    pub fn is_empty(&self) -> bool { self.cells.is_empty() }
    pub fn len(&self) -> usize { self.cells.len() }

    /// Cells whose chunk is now loaded, according to `loaded`, split
    /// out for a retry. What is still unloaded stays deferred.
    pub fn take_ready(&mut self, chunk_size: i32,
                      loaded: &dyn Fn((i32, i32, i32)) -> bool) -> Vec<Change> {
        let (ready, still): (Vec<Change>, Vec<Change>) = self.cells.drain(..)
            .partition(|c| loaded((c.at.0.div_euclid(chunk_size),
                                   c.at.1.div_euclid(chunk_size),
                                   c.at.2.div_euclid(chunk_size))));
        self.cells = still;
        ready
    }

    pub fn message(&self) -> String {
        if self.cells.is_empty() { return "nothing deferred".into() }
        let chunks: std::collections::BTreeSet<(i32, i32, i32)> = self.cells.iter()
            .map(|c| (c.at.0.div_euclid(32), c.at.1.div_euclid(32), c.at.2.div_euclid(32)))
            .collect();
        format!("{} block(s) waiting for {} chunk(s) to load; they place themselves when \
                 you go there, or /deferred drop to forget them",
                self.cells.len(), chunks.len())
    }
}

/// Reconstruct a workset's build sequence from its journal.
///
/// The turns are IR with the anchor they were parsed against, so
/// replaying them against a fresh world rebuilds the thing. This is the
/// recovery for a lost or rolled-back world, and it is also the reason
/// every turn's IR is stored rather than just its prose.
pub fn replay_script(w: &crate::land::Workset) -> String {
    let mut s = format!("# replay {} ({} turns)\n", w.name, w.turns.len());
    for (i, t) in w.turns.iter().enumerate() {
        s.push_str(&format!("# turn {} by {}: {}\n", i + 1, t.player, t.prose));
        s.push_str(t.ir.trim_end());
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::parse_plan_at;

    #[test]
    fn one_bad_line_does_not_lose_the_other_eight() {
        // A 1B that gets eight lines right and one wrong used to get
        // nothing built.
        let ir = "# hut\n\
                  enclose @-4,-4 @4,4 h=4 block=oak_planks\n\
                  roof @-4,-4 @4,4 gable\n\
                  frobnicate the widget\n\
                  tree @8,8 oak h=7\n";
        let (_, issues) = parse_plan_at(ir, Some((0, 65, 0)));
        let s = salvage(ir, &issues);
        assert_eq!(s.bad.len(), 1);
        assert_eq!(s.bad[0].0, 4, "the fourth line");
        assert!(s.bad[0].1.contains("frobnicate"));
        assert!(!s.is_total_loss());
        let (plan, issues2) = parse_plan_at(&s.good, Some((0, 65, 0)));
        assert!(issues2.iter().all(|i| i.severity != Severity::Error));
        assert_eq!(plan.ops.len(), 3, "the good lines run");
    }

    #[test]
    fn the_repair_prompt_shows_only_the_failures() {
        let ir = "enclose @-4,-4 @4,4 h=4\nfrobnicate\ntree @8,8 oak\n";
        let (_, issues) = parse_plan_at(ir, Some((0, 65, 0)));
        let s = salvage(ir, &issues);
        let p = s.repair_prompt();
        assert!(p.contains("frobnicate"));
        assert!(!p.contains("enclose"), "a model shown its whole plan rewrites the parts that were fine");
        assert!(p.contains("Rewrite ONLY"));
        let merged = s.merge("tree @2,2 birch h=6");
        assert!(merged.contains("enclose") && merged.contains("birch"));
        assert!(!merged.contains("frobnicate"));
    }

    #[test]
    fn a_structural_failure_withdraws_the_roof_that_stood_on_it() {
        let (plan, _) = parse_plan_at(
            "enclose @-4,-4 @4,4 h=4\nroof @-4,-4 @4,4 gable\ntree @9,9 oak\n",
            Some((0, 65, 0)));
        // pretend the walls failed but the rest ran
        let ok = [false, true, true];
        let reasons = ["refused: too big".to_string(), String::new(), String::new()];
        let w = plan_rollback(&plan.ops, &ok, &reasons);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].failed_op, 1);
        assert_eq!(w[0].rolled_back, vec![2], "the roof is withdrawn; the tree is not");
        assert!(w[0].message().contains("withdrew op(s) 2"));
    }

    #[test]
    fn a_decorative_failure_withdraws_nothing() {
        let (plan, _) = parse_plan_at(
            "enclose @-4,-4 @4,4 h=4\nscatter @-8,-8 @8,8\nroof @-4,-4 @4,4\n",
            Some((0, 65, 0)));
        let ok = [true, false, true];
        let w = plan_rollback(&plan.ops, &ok, &["".into(), "no ground".into(), "".into()]);
        assert!(w.is_empty(), "a failed scatter is skipped, not propagated");
    }

    #[test]
    fn later_structure_is_kept_after_an_earlier_structural_failure() {
        // Two towers; the first fails. The second stands on its own and
        // must not be withdrawn.
        let (plan, _) = parse_plan_at(
            "cylinder @0,0 r=3 h=8\ncylinder @20,0 r=3 h=8\nroof @18,-2 @22,2\n",
            Some((0, 65, 0)));
        let ok = [false, true, true];
        let w = plan_rollback(&plan.ops, &ok, &["x".into(), "".into(), "".into()]);
        assert_eq!(w[0].rolled_back, vec![3], "the roof is withdrawn, the second tower kept");
    }

    #[test]
    fn deferred_cells_come_back_when_their_chunk_loads() {
        let mut d = Deferred::default();
        d.cells = vec![Change { at: (5, 65, 5), to: 1 }, Change { at: (500, 65, 5), to: 1 }];
        // only the origin chunk is loaded
        let ready = d.take_ready(32, &|c| c == (0, 2, 0));
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].at, (5, 65, 5));
        assert_eq!(d.len(), 1, "the far one is still waiting");
        assert!(d.message().contains("1 block(s) waiting"));
    }

    #[test]
    fn a_workset_journal_replays_into_a_script_that_parses() {
        use crate::land::{Registry, Turn};
        let mut r = Registry::new();
        let id = r.claim("hut", "kess", (0, 64, 0), (8, 70, 8));
        r.record(&id, Turn { player: "kess".into(), prose: "a hut".into(),
                             ir: "enclose @-4,-4 @4,4 h=4 block=oak_planks\n".into(), blocks: 100 },
                 (0, 64, 0), (8, 70, 8));
        r.record(&id, Turn { player: "kess".into(), prose: "add a roof".into(),
                             ir: "roof @-4,-4 @4,4 gable\n".into(), blocks: 60 },
                 (0, 64, 0), (8, 74, 8));
        let script = replay_script(r.get(&id).unwrap());
        assert!(script.contains("# replay hut (2 turns)"));
        let (plan, issues) = parse_plan_at(&script, Some((0, 65, 0)));
        assert!(issues.iter().all(|i| i.severity != Severity::Error), "{issues:?}");
        assert_eq!(plan.ops.len(), 2, "both turns' ops, in order");
    }
}
