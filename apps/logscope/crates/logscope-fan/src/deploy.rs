//! deploy — from "which worker" to what it may do, how wide, and where.
//!
//! Four decisions already existed in this crate as separate, pure, tested modules:
//!
//! | question  | module                     | wired before this file? |
//! |-----------|----------------------------|-------------------------|
//! | WHO       | [`crate::roster`] `staff`  | yes — and it applied only `owns` |
//! | WHAT      | [`crate::queen`] `WorkerSpec.tools` | **no** — parsed, validated, never enforced |
//! | HOW MANY  | [`crate::fan_policy`] `decide` | **no** — no caller anywhere |
//! | WHERE     | [`crate::hive`] `place`    | **no** — no caller anywhere |
//!
//! `docs/12-queen-and-workers.md` opens with the claim that a small model becomes effective by
//! being allowed to do less, and lists CHOICE first among the failures: "a 1B with twelve tools
//! emitted prose inside a tool fence for six turns → `tools` is a whitelist, usually 2–4". The
//! whitelist was written into every spec, checked against the runtime's tool list by `validate`,
//! and then dropped: a staffed worker ran with all thirteen tools, taught all thirteen. Selection
//! did not bind deployment.
//!
//! This module is the join. [`plan`] runs the four decisions in order and returns a
//! [`Deployment`] whose [`trail`](Deployment::trail) says what each stage decided and why, in the
//! order it decided it — so "why is this unconfined", "why only one worker" and "why that machine"
//! are read off a log line instead of reconstructed.
//!
//! ## The one representational trap this exists to hold shut
//!
//! `WorkerSpec.owns == []` means READ-ONLY. `Workspace.owns == []` means UNRESTRICTED. The same
//! value, opposite meanings, on the two sides of one assignment (`cfg.owns = spec.owns.clone()`).
//! [`Lane`] makes the distinction a type, and [`Confinement`] never hands the runtime an empty
//! lane without a tool whitelist that contains no write tool — which `validate` now guarantees
//! for any spec that reaches a roster (`SpecError::WriteWithoutLane`).
//!
//! Pure: facts in, a decision out. No I/O, no model calls, no clock.

use crate::fan_policy::{self, Bound, Capacity};
use crate::hive::{self, Cell, Job, Placement};
use crate::queen::{WorkerSpec, LANE_GOVERNED_TOOLS};
use crate::roster::{Roster, Staffing};
use crate::workerkit::Reach;

/// Where a staffed worker may write. There is no `Unrestricted` variant on purpose: a worker that
/// came out of a roster is confined by construction, and an unconfined run is the ABSENCE of a
/// [`Confinement`], not a value of one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lane {
    /// Reads and reports. No lane-governed tool is in its whitelist.
    ReadOnly,
    /// May write under these paths and nowhere else.
    Paths(Vec<String>),
}

/// Everything a run needs in order to BE the selected worker rather than a general agent wearing
/// its name: the lane, the tool whitelist, and the three lists the prompt should carry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Confinement {
    pub worker: String,
    pub lane: Lane,
    /// The whitelist, in the spec's order. Never empty (`validate` refuses a tool-less spec).
    pub tools: Vec<String>,
    pub context: Vec<String>,
    pub constraints: Vec<String>,
    pub success: Vec<String>,
}

impl Confinement {
    pub fn of(spec: &WorkerSpec) -> Confinement {
        let paths: Vec<String> = spec.owns.iter()
            .map(|o| o.trim().trim_end_matches('/').to_string())
            .filter(|o| !o.is_empty()).collect();
        // Defence in depth behind `validate`: a spec that reached here with a write tool and no
        // lane loses the write tool rather than gaining the repository.
        let tools: Vec<String> = spec.tools.iter()
            .filter(|t| !paths.is_empty() || !LANE_GOVERNED_TOOLS.contains(&t.as_str()))
            .cloned().collect();
        Confinement {
            worker: spec.name.clone(),
            lane: if paths.is_empty() { Lane::ReadOnly } else { Lane::Paths(paths) },
            tools,
            context: spec.context.clone(),
            constraints: spec.constraints.clone(),
            success: spec.success.clone(),
        }
    }

    /// The lane in the RUNTIME's representation, where empty means unrestricted. Safe to hand over
    /// only together with [`Confinement::tools`] — which is why the two are one value and the
    /// caller applies both or neither.
    pub fn runtime_owns(&self) -> Vec<String> {
        match &self.lane { Lane::ReadOnly => Vec::new(), Lane::Paths(p) => p.clone() }
    }

    /// A lane AND a tool that can use it. A lane-holder whitelisted only for reading cannot put a
    /// byte on disk, so it is a reader for every purpose that matters (fanning, above all).
    pub fn may_write(&self) -> bool {
        matches!(self.lane, Lane::Paths(_)) && self.tools.iter().any(|t| LANE_GOVERNED_TOOLS.contains(&t.as_str()))
    }

    /// One line for a HUD: who, where, with what.
    pub fn describe(&self) -> String {
        let lane = match &self.lane {
            Lane::ReadOnly => "read-only".to_string(),
            Lane::Paths(p) => format!("writes only in {}", p.join(", ")),
        };
        format!("{} — {lane}; tools: {}", self.worker, self.tools.join(", "))
    }
}

/// The facts [`plan`] decides from. Everything the four stages need, and nothing they must fetch.
pub struct Request<'a> {
    pub task: &'a str,
    pub roster: &'a Roster,
    /// The hive's cells as currently known; a single-box session is one cell.
    pub matrix: &'a [Cell],
    /// How wide the caller (or a model) would like to go. 1 for an ordinary `:agent` run.
    pub proposed_width: u16,
    /// Distinct things a fan could explore — branches, files, hypotheses. 1 = nothing to fan over.
    pub possibilities: u16,
    pub budget_tokens: u32,
    pub capacity: Capacity,
    /// Prompt size in tokens — what a cold cell would have to prefill.
    pub prompt_tokens: usize,
    /// Turns the run may take (the agent's step budget). See [`hive::Job::turns`].
    pub expected_turns: u32,
    /// Model labels acceptable for this run; empty = whatever is loaded.
    pub models: Vec<String>,
}

/// What to do with the task.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// Run it, confined as [`Deployment::confinement`] says (or unconfined when that is `None`).
    Run,
    /// The rules cannot choose between these workers. A Queen call, not a coin flip.
    AskQueen { candidates: Vec<String> },
    /// Nothing can run it right now; the string is `hive::explain_empty`'s.
    NoCell(String),
}

#[derive(Clone, Debug)]
pub struct Deployment {
    pub verdict: Verdict,
    pub staffing: Staffing,
    /// `Some` exactly when an existing worker was selected. `None` on `Create` is an UNCONFINED
    /// run and the trail says so — that is a fact the operator should see, not infer.
    pub confinement: Option<Confinement>,
    pub width: u16,
    pub width_bound: Bound,
    /// One per worker actually placed; may be shorter than `width` when cells ran out.
    pub placements: Vec<Placement>,
    /// What each stage decided, in order. The point of the module.
    pub trail: Vec<String>,
}

impl Deployment {
    pub fn render(&self) -> String { self.trail.join("\n") }
}

/// Reach for a run that came out of a roster. Every staffed worker is a thinker in `workerkit`'s
/// sense — whole replies, not token-interleaved with a partner — so it MAY run anywhere. What
/// distance costs it is turns × round trip, which is [`hive::Job::turns`]' business, not a
/// reason to forbid the placement.
pub fn reach_of(_c: Option<&Confinement>) -> Reach { Reach::Anywhere }

/// Run the four decisions. Never panics, never blocks; an empty matrix or roster is an answer.
pub fn plan(req: &Request) -> Deployment {
    let mut trail = Vec::new();

    // 1. WHO
    let staffing = req.roster.staff(req.task);
    let confinement = match &staffing {
        Staffing::Reuse { worker, why } => match req.roster.get(worker) {
            Some(spec) => {
                let c = Confinement::of(spec);
                trail.push(format!("who: {worker} ({why})"));
                // 2. WHAT
                trail.push(format!("what: {}", c.describe()));
                if c.tools.len() < spec.tools.len() {
                    trail.push("what: write tools DROPPED — the spec names them but owns no lane to write in".into());
                }
                Some(c)
            }
            None => { trail.push(format!("who: {worker} matched but is not in the roster — running unconfined")); None }
        },
        Staffing::Ambiguous { candidates } => {
            trail.push(format!("who: ambiguous between {} — equally specific lanes; this is a Queen call", candidates.join(", ")));
            None
        }
        Staffing::Create => {
            trail.push("who: no worker's lane covers this — UNCONFINED run (all tools, whole repository); :queen the task if it recurs".into());
            None
        }
    };
    if let Staffing::Ambiguous { candidates } = &staffing {
        return Deployment { verdict: Verdict::AskQueen { candidates: candidates.clone() }, staffing,
            confinement: None, width: 0, width_bound: Bound::AsProposed, placements: Vec::new(), trail };
    }

    // 3. HOW MANY — a worker that may write is never fanned over one lane: n writers in the same
    // paths is a merge conflict the lane was supposed to prevent. Readers fan freely.
    let writer = confinement.as_ref().is_some_and(|c| c.may_write());
    let (proposed, possibilities) = if writer && req.proposed_width > 1 {
        trail.push(format!("how many: proposed {} but this worker WRITES in one lane — one writer per lane", req.proposed_width));
        (1, 1)
    } else { (req.proposed_width.max(1), req.possibilities.max(1)) };
    let (width, width_bound) = fan_policy::decide(proposed, possibilities, req.budget_tokens, req.capacity);
    trail.push(format!("how many: {width} (bound: {width_bound:?}; proposed {proposed}, {possibilities} possibilities, {} tokens, {} of {} slots free)",
        req.budget_tokens, req.capacity.free(), req.capacity.slots));

    // 4. WHERE
    let job = Job { reach: reach_of(confinement.as_ref()), partner_node: None, prompt_tokens: req.prompt_tokens,
                    models: req.models.clone(), turns: req.expected_turns.max(1) };
    let placements = if width <= 1 { hive::place(&job, req.matrix).into_iter().collect::<Vec<_>>() }
                     else { hive::place_fan(&job, req.matrix, width as usize) };
    if placements.is_empty() {
        let why = hive::explain_empty(&job, req.matrix);
        trail.push(format!("where: nowhere — {why}"));
        return Deployment { verdict: Verdict::NoCell(why), staffing, confinement, width, width_bound, placements, trail };
    }
    for p in &placements { trail.push(format!("where: {}", p.reason)); }
    if placements.len() < width as usize {
        trail.push(format!("where: only {} of {width} workers found a cell — the rest would queue", placements.len()));
    }
    Deployment { verdict: Verdict::Run, staffing, confinement, width, width_bound, placements, trail }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hive::Node;
    use crate::queen::Persistence;
    use crate::roster::seed_roster;

    fn spec(name: &str, owns: &[&str], tools: &[&str]) -> WorkerSpec {
        WorkerSpec { name: name.into(), purpose: "p".into(),
            owns: owns.iter().map(|s| s.to_string()).collect(), context: vec![],
            tools: tools.iter().map(|s| s.to_string()).collect(), constraints: vec![],
            success: vec!["cargo test passes".into()], persistence: Persistence::OneShot }
    }
    fn cell(node: &str, rtt: u32, tok_s: f32, warm: usize) -> Cell {
        Cell { node: Node { id: node.into(), rtt_ms: rtt, tok_s }, model: "m".into(), warm_prefix: warm, busy: false }
    }
    fn req<'a>(task: &'a str, roster: &'a Roster, matrix: &'a [Cell]) -> Request<'a> {
        Request { task, roster, matrix, proposed_width: 1, possibilities: 1, budget_tokens: 8000,
                  capacity: Capacity { slots: 1, busy: 0 }, prompt_tokens: 1600, expected_turns: 12, models: vec![] }
    }

    /// The defect this module was written for, as a test: selecting a worker must bring its WHOLE
    /// confinement, not just the lane.
    #[test]
    fn a_staffed_worker_arrives_with_its_whitelist_not_just_its_lane() {
        let roster = seed_roster();
        let m = [cell("local", 0, 20.0, 0)];
        let d = plan(&req("tidy docs/12-queen-and-workers.md", &roster, &m));
        assert_eq!(d.verdict, Verdict::Run);
        let c = d.confinement.clone().expect("doc-writer owns docs/");
        assert_eq!(c.worker, "doc-writer");
        assert_eq!(c.lane, Lane::Paths(vec!["docs".into()]));
        assert_eq!(c.tools, ["read_file", "read_near", "write_file", "doc"]);
        assert!(!c.tools.iter().any(|t| t == "run" || t == "make_tool"), "a doc writer has no shell");
        let t = d.render();
        assert!(t.contains("who: doc-writer") && t.contains("what: doc-writer — writes only in docs; tools: read_file"), "{t}");
    }

    /// The representational trap. A read-only spec's empty `owns` must never reach the runtime as
    /// "unrestricted" with a write tool still on the menu.
    #[test]
    fn read_only_is_a_lane_not_the_absence_of_one() {
        let c = Confinement::of(&seed_roster().get("reader").unwrap().clone());
        assert_eq!(c.lane, Lane::ReadOnly);
        assert!(c.runtime_owns().is_empty(), "the runtime's spelling of read-only IS empty …");
        assert!(!c.tools.iter().any(|t| LANE_GOVERNED_TOOLS.contains(&t.as_str())), "… which is only safe because no write tool is whitelisted");
        assert!(!c.may_write());
        // A spec that slipped past validate with a write tool and no lane loses the tool.
        let bad = spec("loose", &[], &["read_file", "write_file", "edit"]);
        assert!(matches!(bad.validate(&["read_file", "write_file", "edit"]), Err(crate::queen::SpecError::WriteWithoutLane(t)) if t == "write_file"));
        let c = Confinement::of(&bad);
        assert_eq!(c.tools, ["read_file"]);
        // and whitespace / bare-slash lanes are not lanes
        assert_eq!(Confinement::of(&spec("blank", &[" ", "/"], &["read_file", "edit"])).lane, Lane::ReadOnly);
    }

    #[test]
    fn validate_refuses_a_tool_less_spec_because_empty_would_deploy_as_everything() {
        let e = spec("thinker", &["src"], &[]).validate(&["read_file"]).unwrap_err();
        assert_eq!(e, crate::queen::SpecError::MissingField("tools"));
        assert!(e.correction().contains("tools"));
        // every seed still validates under the two new refusals
        for n in seed_roster().names() {
            let roster = seed_roster();
            let s = roster.get(n).unwrap();
            assert!(s.validate(&["write_file", "edit", "read_file", "read_range", "read_near", "list_dir", "search",
                                 "run", "git_log", "doc", "propose", "scratch_code", "make_tool"]).is_ok(), "{n}");
        }
    }

    /// Unconfined is a fact the operator should SEE. `Create` is the ordinary case for a prose
    /// task, so it is not an error — but it is on the trail, in words.
    #[test]
    fn an_unstaffed_task_says_it_is_unconfined() {
        let roster = seed_roster();
        let m = [cell("local", 0, 20.0, 0)];
        let d = plan(&req("make the renderer faster", &roster, &m));
        assert_eq!(d.staffing, Staffing::Create);
        assert!(d.confinement.is_none());
        assert_eq!(d.verdict, Verdict::Run);
        assert!(d.trail[0].contains("UNCONFINED"), "{:?}", d.trail);
    }

    #[test]
    fn ambiguity_stops_the_pipeline_before_width_and_placement() {
        let mut roster = Roster::new();
        roster.add(spec("ui-a", &["src/ui"], &["read_file", "edit"]));
        roster.add(spec("ui-b", &["src/ui"], &["read_file", "edit"]));
        let m = [cell("local", 0, 20.0, 0)];
        let d = plan(&req("fix src/ui/panel.rs", &roster, &m));
        assert_eq!(d.verdict, Verdict::AskQueen { candidates: vec!["ui-a".into(), "ui-b".into()] });
        assert!(d.placements.is_empty() && d.width == 0, "nothing downstream ran on an undecided WHO");
        assert_eq!(d.trail.len(), 1);
    }

    /// One writer per lane. A fan of writers in the same paths is the merge conflict the lane was
    /// for; a fan of READERS is fine and goes as wide as fan_policy allows.
    #[test]
    fn writers_are_not_fanned_over_their_own_lane_and_readers_are() {
        let mut roster = Roster::new();
        roster.add(spec("ui", &["src/ui"], &["read_file", "edit"]));
        roster.add(spec("ui-reader", &["src/view"], &["read_file", "search"])); // lane routes to it; no write tool
        let m = [cell("a", 0, 20.0, 0), cell("b", 5, 20.0, 0), cell("c", 5, 20.0, 0)];
        let wide = |task| { let mut r = req(task, &roster, &m); r.proposed_width = 3; r.possibilities = 3;
                            r.capacity = Capacity { slots: 3, busy: 0 }; plan(&r) };
        let w = wide("fix src/ui/panel.rs");
        assert_eq!((w.width, w.placements.len()), (1, 1));
        assert!(w.render().contains("one writer per lane"), "{}", w.render());
        // ui-reader holds a lane but no tool that can write in it: it cannot collide with itself,
        // so it fans. (My first draft held it to one on the lane alone — wrong: the hazard is two
        // WRITES, and a whitelist without write_file/edit cannot make one.)
        let r = wide("survey src/view/list.rs");
        assert_eq!(r.confinement.as_ref().map(|c| c.worker.as_str()), Some("ui-reader"));
        assert_eq!(r.width, 3);
        // an unstaffed (Create) reading task fans
        let f = wide("compare the three renderers");
        assert_eq!((f.width, f.width_bound, f.placements.len()), (3, Bound::AsProposed, 3));
    }

    /// WHERE: the hive's cost model counts a round trip per TURN now. A remote node that is three
    /// times faster wins a one-shot thinker and loses a 24-step tool loop to the local box.
    #[test]
    fn a_tool_loop_stays_near_and_a_single_reply_may_travel() {
        let roster = seed_roster();
        let m = [cell("local", 0, 20.0, 0), cell("far", 150, 60.0, 0)];
        let mut r = req("make the renderer faster", &roster, &m);
        r.prompt_tokens = 800;   // local prefill 5.0 s, far 1.67 s: far is 3.3 s ahead before the link
        r.expected_turns = 1;
        assert_eq!(plan(&r).placements[0].cell.node.id, "far", "one reply: 0.15 s of link does not pay back 3.3 s");
        r.expected_turns = 24;
        let d = plan(&r);
        assert_eq!(d.placements[0].cell.node.id, "local", "24 turns: 3.6 s of link is more than the prefill saved");
        // and warmth still beats both: a warm far cell skips the prefill entirely
        let m2 = [cell("local", 0, 20.0, 0), cell("far", 150, 60.0, 800)];
        r.matrix = &m2; r.expected_turns = 24;
        assert_eq!(plan(&r).placements[0].cell.node.id, "far");
    }

    #[test]
    fn no_cell_is_an_answer_with_a_reason() {
        let roster = seed_roster();
        let mut busy = cell("local", 0, 20.0, 0); busy.busy = true;
        let m = [busy];
        let d = plan(&req("make the renderer faster", &roster, &m));
        assert!(matches!(&d.verdict, Verdict::NoCell(w) if w.contains("busy")), "{:?}", d.verdict);
        assert!(d.render().contains("where: nowhere"));
        let none: [Cell; 0] = [];
        assert!(matches!(plan(&req("x", &roster, &none)).verdict, Verdict::NoCell(w) if w.contains("empty")));
    }
}
