//! job — a big build as a sequence of small requests, each of which fits.
//!
//! ## What the log said
//!
//! ```text
//! the reply ran out of room at 5200 tokens; using the 0 pass(es) that finished
//! that did not compose -- trying the direct path
//! the model replied (1149 chars) but produced no plan lines:
//!   "asset trunk box w=-5 h=-20 d=-15 material=brownwood hollow rel trunk on ground by 2 repeat"
//! Program 'add leaves and branches and vines and make this tree way bigger...'
//!   -- 1 pass(es), 1653 cells: mass (add 1653)
//! ```
//!
//! Four separate failures, and only the first is about context size:
//!
//! 1. **The reply was cut off and NOTHING survived.** `build_loop::turn` is
//!    one-shot: one reply carries the whole build, so a truncation salvages
//!    "the 0 pass(es) that finished". The model had written 5200 tokens of
//!    real work and every one was discarded.
//! 2. **The fallback lost the structure.** "trying the direct path" produced
//!    1653 cells of undifferentiated `mass` — a shape, with no parts, which
//!    is how the tree ended up a single-material lollipop (see `scheme`).
//! 3. **Negative dimensions.** `w=-5 h=-20 d=-15` — a box with negative
//!    extents is not a small box, it is an empty one or an inverted one.
//! 4. **A material that does not exist.** `brownwood` is not in
//!    `palette::BLOCKS`; `oak_log` is. `palette::resolve_or_suggest` already
//!    handles exactly this and was not consulted.
//!
//! This module addresses 1 and 2. `scheme` addresses the material half of 2,
//! and [`StepFault`] below covers 3 and 4 at the point a step is accepted.
//!
//! ## The fix is smaller requests, not a bigger model
//!
//! A [`Job`] is an ordered list of [`Step`]s, each one narrow enough to
//! finish: "the trunk", then "the canopy", then "the branches". Each step is
//! committed on its own, so a step that overruns costs THAT step and not the
//! whole tree. The log's request — "add leaves and branches and vines and
//! make this tree way bigger taller thicker and more like avatar pandora
//! trees" — is five requests in a trench coat, and asking it as one is what
//! blew the budget.
//!
//! ## Readback must be bounded, or it becomes the next context problem
//!
//! The obvious way to tell the model what already exists is to send it back.
//! That is self-defeating: by step four the readback IS the context overflow.
//! [`Job::readback`] is therefore **O(steps), not O(blocks)** — per-step
//! counts, a bounding box, and the materials in use. A 1653-block trunk and a
//! 16-block one produce the same size of readback. There is a test for that,
//! because it is the property that makes the whole approach work.

use std::collections::BTreeMap;

use crate::palette;
use crate::scheme::{Archetype, Role};

/// Rough token budget for one reply.
///
/// 5200 is what the observed model actually ran out at, not a round number
/// picked for looking sensible. A step is planned well under it: the reply
/// carries prose as well as plan lines, and a step that exactly fills the
/// budget is a step that truncates.
pub const REPLY_TOKENS: u32 = 5200;

/// Target for one step's plan lines, leaving room for the model's prose,
/// the system prompt and the readback.
pub const STEP_TOKENS: u32 = 1200;

/// Where a step got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    Pending,
    /// Asked and accepted; blocks are in the world.
    Done,
    /// Asked, and the reply was unusable. Carries nothing — the reason lives
    /// in the log; what matters here is that it can be retried without
    /// redoing its predecessors.
    Failed,
    /// Deliberately not attempted (an optional role the player declined).
    Skipped,
}

impl StepState {
    pub fn finished(self) -> bool {
        !matches!(self, StepState::Pending)
    }
    /// Skipped counts as success — the same rule `studio`'s pipeline uses.
    /// An optional part nobody asked for must not stall the job.
    pub fn ok(self) -> bool {
        matches!(self, StepState::Done | StepState::Skipped)
    }
}

/// One narrow request.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    /// What part of the object this builds, when it maps to one.
    pub role: Option<Role>,
    /// The instruction, phrased narrowly enough to finish.
    pub goal: String,
    /// Whether the job can proceed without it.
    pub required: bool,
    pub state: StepState,
    /// Blocks this step actually placed.
    pub placed: usize,
    /// Distinct block ids it used, for the readback and for `scheme` checks.
    pub blocks: Vec<u16>,
}

impl Step {
    pub fn new(role: Option<Role>, goal: impl Into<String>, required: bool) -> Step {
        Step { role, goal: goal.into(), required, state: StepState::Pending, placed: 0, blocks: Vec::new() }
    }
}

/// Why a step's reply was refused.
///
/// These are the malformed-output cases from the log, caught at acceptance so
/// a bad step fails cleanly and retries instead of poisoning the build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepFault {
    /// `w=-5 h=-20 d=-15`. A negative extent is not a small box.
    NegativeExtent { field: &'static str, value: i32 },
    /// `material=brownwood`. Carries the catalogue's nearest suggestion,
    /// since `palette` can almost always name one.
    UnknownMaterial { given: String, suggestion: Option<&'static str> },
    /// The step produced nothing at all.
    Empty,
}

impl StepFault {
    /// An instruction the model can act on, naming the fix.
    pub fn explain(&self) -> String {
        match self {
            StepFault::NegativeExtent { field, value } => format!(
                "{field}={value} is negative — extents are sizes, so use {} and move the \
                 anchor if you meant the other direction",
                value.abs().max(1)),
            StepFault::UnknownMaterial { given, suggestion } => match suggestion {
                Some(s) => format!("`{given}` is not a block in this world — did you mean `{s}`?"),
                None => format!("`{given}` is not a block in this world; name one from the catalogue"),
            },
            StepFault::Empty => "that step placed no blocks; say what to build and where".into(),
        }
    }
}

/// Check one step's proposed dimensions and material before accepting it.
///
/// Free function rather than a method: the parser can call it on a single
/// line without a `Job` in hand, which is where the log's bad line would have
/// been caught.
pub fn check_line(w: i32, h: i32, d: i32, material: &str) -> Vec<StepFault> {
    let mut out = Vec::new();
    for (field, v) in [("w", w), ("h", h), ("d", d)] {
        if v < 0 {
            out.push(StepFault::NegativeExtent { field, value: v });
        }
    }
    if !material.is_empty() && palette::by_name(material).is_none() {
        // resolve_or_suggest is the existing machinery for exactly this and
        // was never consulted on the path that produced `brownwood`.
        let suggestion = palette::suggest(material, 1).first().map(|(b, _)| b.name);
        out.push(StepFault::UnknownMaterial { given: material.to_string(), suggestion });
    }
    out
}

/// A big build, as steps.
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub what: String,
    pub arch: Archetype,
    pub steps: Vec<Step>,
    /// World-space extent of everything placed so far, for the readback.
    bounds: Option<([i32; 3], [i32; 3])>,
}

impl Job {
    /// Split a request into steps.
    ///
    /// Structure comes from the archetype's roles (`scheme::Archetype::steps`)
    /// so the job and the material scheme agree on what the parts ARE — the
    /// tree's trunk and canopy are one list of parts, not two. A massing step
    /// leads, because the later parts are placed relative to it.
    pub fn plan(what: impl Into<String>, arch: Archetype) -> Job {
        let what = what.into();
        let mut steps = vec![Step::new(None, format!("the overall shape and size of {what}, in one cheap material"), true)];
        let required = arch.required();
        for (role, question, _) in arch.steps() {
            let req = required.contains(&role);
            steps.push(Step::new(
                Some(role),
                format!("just the {} of {what}. {question}", role.name()),
                req,
            ));
        }
        Job { what, arch, steps, bounds: None }
    }

    /// The next step to ask about, or `None` when the job is done.
    ///
    /// A FAILED step does not stop the job the way a failed stage stops a
    /// pipeline: the parts are largely independent, and a tree with no vines
    /// is still a tree. Required steps that failed are reported by
    /// [`Self::unfinished`] instead, so the player can retry them.
    pub fn next(&self) -> Option<usize> {
        self.steps.iter().position(|s| s.state == StepState::Pending)
    }

    /// Required steps that did not succeed.
    pub fn unfinished(&self) -> Vec<&Step> {
        self.steps.iter().filter(|s| s.required && !s.state.ok()).collect()
    }

    pub fn done(&self) -> bool {
        self.next().is_none()
    }

    /// 0..=1 across all steps.
    pub fn progress(&self) -> f32 {
        if self.steps.is_empty() {
            return 1.0;
        }
        self.steps.iter().filter(|s| s.state.finished()).count() as f32 / self.steps.len() as f32
    }

    /// Record a step's result. `min`/`max` extend the job's bounds.
    pub fn record(&mut self, i: usize, placed: usize, blocks: &[u16],
                  extent: Option<([i32; 3], [i32; 3])>) {
        let Some(s) = self.steps.get_mut(i) else { return };
        s.placed = placed;
        s.blocks = {
            let mut v = blocks.to_vec();
            v.sort_unstable();
            v.dedup();
            v
        };
        // A step that placed nothing is a failure however cheerfully the
        // model reported it -- the log's "0 pass(es)" dressed as success is
        // exactly this.
        s.state = if placed == 0 { StepState::Failed } else { StepState::Done };
        if let Some((lo, hi)) = extent {
            self.bounds = Some(match self.bounds {
                None => (lo, hi),
                Some((a, b)) => (
                    [a[0].min(lo[0]), a[1].min(lo[1]), a[2].min(lo[2])],
                    [b[0].max(hi[0]), b[1].max(hi[1]), b[2].max(hi[2])],
                ),
            });
        }
    }

    pub fn fail(&mut self, i: usize) {
        if let Some(s) = self.steps.get_mut(i) {
            s.state = StepState::Failed;
        }
    }

    pub fn skip(&mut self, i: usize) {
        if let Some(s) = self.steps.get_mut(i) {
            s.state = StepState::Skipped;
        }
    }

    /// What to tell the model about what already exists.
    ///
    /// **Bounded by construction.** One line per step plus one bounds line —
    /// O(steps), never O(blocks). Sending the placed blocks back would make
    /// the readback the next context overflow, which is the trap this whole
    /// module exists to avoid.
    ///
    /// Names materials by block NAME rather than id: the model answers in
    /// names, so a readback in ids asks it to translate, and `brownwood`
    /// suggests it is not good at that.
    pub fn readback(&self) -> String {
        let mut out = vec![format!("building: {} ({} of {} steps done)",
            self.what,
            self.steps.iter().filter(|s| s.state.finished()).count(),
            self.steps.len())];
        for s in &self.steps {
            let part = s.role.map(|r| r.name()).unwrap_or("shape");
            match s.state {
                StepState::Done => {
                    let mats: Vec<&str> = s.blocks.iter()
                        .filter_map(|id| palette::by_id(*id).map(|b| b.name))
                        .collect();
                    out.push(format!("  {part}: done, {} blocks{}", s.placed,
                        if mats.is_empty() { String::new() } else { format!(" of {}", mats.join(", ")) }));
                }
                StepState::Failed => out.push(format!("  {part}: FAILED, retry or skip")),
                StepState::Skipped => out.push(format!("  {part}: skipped")),
                StepState::Pending => out.push(format!("  {part}: not yet built")),
            }
        }
        if let Some((lo, hi)) = self.bounds {
            out.push(format!("  extent: {},{},{} to {},{},{}",
                lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]));
        }
        out.join("\n")
    }

    /// Materials used so far, by role — the input `scheme::Scheme` validation
    /// needs to catch a build going monotone ACROSS steps rather than within
    /// one.
    pub fn materials_by_role(&self) -> BTreeMap<Role, Vec<u16>> {
        let mut m: BTreeMap<Role, Vec<u16>> = BTreeMap::new();
        for s in &self.steps {
            if let Some(r) = s.role {
                if s.state == StepState::Done {
                    m.entry(r).or_default().extend(s.blocks.iter().copied());
                }
            }
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: &str) -> u16 {
        palette::by_name(n).unwrap().id
    }

    /// The log's request was five requests in one. A job splits it so each
    /// piece can finish inside a reply.
    #[test]
    fn a_tree_job_asks_for_the_trunk_and_the_canopy_separately() {
        let j = Job::plan("a tree", Archetype::Tree);
        assert!(j.steps.len() >= 3, "shape + trunk + canopy at least");
        let roles: Vec<Option<Role>> = j.steps.iter().map(|s| s.role).collect();
        assert_eq!(roles[0], None, "massing leads -- later parts anchor on it");
        assert!(roles.contains(&Some(Role::Trunk)));
        assert!(roles.contains(&Some(Role::Canopy)));
        // Each goal is narrow and says so.
        assert!(j.steps[1].goal.starts_with("just the"), "{}", j.steps[1].goal);
    }

    /// THE CUT-OFF BUG. 5200 tokens of work discarded because one reply had
    /// to carry everything. Earlier steps must survive a later failure.
    #[test]
    fn a_failed_step_does_not_discard_the_finished_ones() {
        let mut j = Job::plan("a tree", Archetype::Tree);
        j.record(0, 1653, &[id("oak_log")], Some(([0, 0, 0], [8, 20, 8])));
        let trunk = j.steps.iter().position(|s| s.role == Some(Role::Trunk)).unwrap();
        j.record(trunk, 200, &[id("oak_log")], None);
        let canopy = j.steps.iter().position(|s| s.role == Some(Role::Canopy)).unwrap();
        j.fail(canopy);

        assert_eq!(j.steps[0].state, StepState::Done, "the massing survived");
        assert_eq!(j.steps[trunk].placed, 200);
        // And the job keeps going rather than halting on the failure.
        assert!(j.next().is_some(), "remaining steps are still offered");
        assert_eq!(j.unfinished().len(), 1, "the canopy is reported for retry");
    }

    /// THE PROPERTY THE WHOLE APPROACH RESTS ON. If the readback grew with
    /// the build, it would become the next context overflow by step four.
    #[test]
    fn the_readback_is_bounded_by_steps_not_by_blocks() {
        let mut small = Job::plan("a tree", Archetype::Tree);
        let mut huge = Job::plan("a tree", Archetype::Tree);
        small.record(0, 16, &[id("oak_log")], Some(([0, 0, 0], [2, 2, 2])));
        huge.record(0, 1_653_000, &[id("oak_log")], Some(([0, 0, 0], [900, 900, 900])));
        let (a, b) = (small.readback(), huge.readback());
        // Same shape of message; only the numerals differ.
        assert_eq!(a.lines().count(), b.lines().count());
        assert!(b.len() < a.len() + 40, "readback grew with the build: {} vs {}", a.len(), b.len());
        assert!(b.len() < 600, "a readback must stay small: {} chars", b.len());
    }

    /// Readback names materials, because the model answers in names — and
    /// `brownwood` shows it is poor at translating ids.
    #[test]
    fn the_readback_names_materials_not_ids() {
        let mut j = Job::plan("a tree", Archetype::Tree);
        j.record(0, 10, &[id("oak_log"), id("leaves")], None);
        let r = j.readback();
        assert!(r.contains("oak_log") && r.contains("leaves"), "{r}");
        assert!(!r.contains("17"), "no raw ids: {r}");
    }

    /// A step that placed nothing is a failure, however cheerfully reported.
    /// "using the 0 pass(es) that finished" was logged as progress.
    #[test]
    fn a_step_that_placed_nothing_is_a_failure() {
        let mut j = Job::plan("a tree", Archetype::Tree);
        j.record(0, 0, &[], None);
        assert_eq!(j.steps[0].state, StepState::Failed);
        assert!(j.readback().contains("FAILED"));
    }

    /// `w=-5 h=-20 d=-15` from the log. A negative extent is not a small box.
    #[test]
    fn negative_extents_are_refused_with_the_fix_named() {
        let faults = check_line(-5, -20, -15, "oak_log");
        assert_eq!(faults.len(), 3, "{faults:?}");
        assert!(faults.contains(&StepFault::NegativeExtent { field: "h", value: -20 }));
        let msg = faults[0].explain();
        assert!(msg.contains("negative") && msg.contains('5'), "{msg}");
        // A valid line is clean.
        assert!(check_line(5, 20, 15, "oak_log").is_empty());
    }

    /// `material=brownwood` from the log. The catalogue can name the fix and
    /// was never asked.
    #[test]
    fn an_unknown_material_is_refused_and_a_real_one_suggested() {
        let faults = check_line(5, 5, 5, "brownwood");
        assert_eq!(faults.len(), 1, "{faults:?}");
        match &faults[0] {
            StepFault::UnknownMaterial { given, suggestion } => {
                assert_eq!(given, "brownwood");
                assert!(suggestion.is_some(), "palette should name something for 'brownwood'");
            }
            f => panic!("wrong fault: {f:?}"),
        }
        assert!(faults[0].explain().contains("brownwood"));
    }

    /// Skipping an optional part must not stall the job.
    #[test]
    fn a_skipped_optional_step_counts_as_finished() {
        let mut j = Job::plan("a tree", Archetype::Tree);
        let opt = j.steps.iter().position(|s| !s.required).expect("an optional step");
        j.skip(opt);
        assert!(j.steps[opt].state.ok());
        assert!(!j.unfinished().iter().any(|s| s.role == j.steps[opt].role));
    }

    #[test]
    fn progress_runs_from_zero_to_one() {
        let mut j = Job::plan("a tree", Archetype::Tree);
        assert_eq!(j.progress(), 0.0);
        assert!(!j.done());
        for i in 0..j.steps.len() {
            j.record(i, 1, &[id("oak_log")], None);
        }
        assert_eq!(j.progress(), 1.0);
        assert!(j.done());
        assert!(j.next().is_none());
    }

    /// Bounds accumulate across steps, so the readback's extent describes the
    /// whole object rather than the last piece.
    #[test]
    fn bounds_accumulate_over_steps() {
        let mut j = Job::plan("a tree", Archetype::Tree);
        j.record(0, 10, &[id("oak_log")], Some(([0, 0, 0], [4, 10, 4])));
        j.record(1, 10, &[id("leaves")], Some(([-6, 8, -6], [10, 22, 10])));
        let r = j.readback();
        assert!(r.contains("-6,8,-6 to 10,22,10"), "{r}");
    }

    /// The job's parts and the scheme's roles must be the same list, or a
    /// build satisfies one and not the other.
    #[test]
    fn job_roles_cover_the_archetypes_required_roles() {
        for arch in [Archetype::Tree, Archetype::Building, Archetype::Tower] {
            let j = Job::plan("x", arch);
            for r in arch.required() {
                assert!(j.steps.iter().any(|s| s.role == Some(*r)),
                    "{arch:?} job has no step for {r:?}");
            }
        }
    }

    /// materials_by_role feeds scheme validation, so a build that goes
    /// monotone ACROSS steps is catchable too -- the lollipop, assembled one
    /// step at a time.
    #[test]
    fn materials_by_role_reports_only_completed_steps() {
        let mut j = Job::plan("a tree", Archetype::Tree);
        let trunk = j.steps.iter().position(|s| s.role == Some(Role::Trunk)).unwrap();
        let canopy = j.steps.iter().position(|s| s.role == Some(Role::Canopy)).unwrap();
        j.record(trunk, 5, &[id("oak_log")], None);
        j.fail(canopy);
        let m = j.materials_by_role();
        assert_eq!(m.get(&Role::Trunk), Some(&vec![id("oak_log")]));
        assert_eq!(m.get(&Role::Canopy), None, "a failed step contributes nothing");
    }
}
