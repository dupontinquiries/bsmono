//! The chat loop: what happens between Enter and blocks appearing.
//!
//! This is the sequencing layer. It owns no UI and no model; it owns
//! the ORDER in which the pieces run, and the state between turns.
//! The game loop feeds it three things -- the parsed command, the
//! current gaze, and a model round-trip -- and reads back what to
//! show and what to commit.
//!
//!   /build <prose>
//!       |
//!       v
//!   Focus::from_gaze(gaze)              where are they looking, at what scale
//!       |
//!       v
//!   read_hint + read_region digest      the model's "open the file first"
//!       |  (one model round-trip: prose + context -> line IR)
//!       v
//!   parse_plan_at(ir, anchor)           @dx,dz resolves against the focus
//!       |
//!       v
//!   validate + check_math               static errors block; suspects warn
//!       |
//!       v
//!   run_plan(dry_run=true)              exact preview, world untouched
//!       |
//!       v
//!   PENDING: show echo + preview        user reads "9 blocks across"
//!       |
//!   /commit                             or /undo the last one
//!       |
//!       v
//!   run_plan(dry_run=false) -> batches  chunk-grouped, for the mesher
//!
//! The pending plan is tied to the feature id it was made for. If the
//! player walks away and the focus changes tier, the pending plan is
//! stale: its @coordinates were relative to somewhere else. The loop
//! refuses to commit a stale plan and says why, rather than building
//! it in the wrong place.

use crate::focus::{Focus, Gaze, ScaleTier};
use crate::prompt::{user_turn, Pass};
use crate::critique;
use crate::anim::Profile;
use crate::voice::VoiceIntent;
use crate::math::{check_math, explain_plan, Note};
use crate::plan::{parse_plan_at, run_plan, validate, Batch, BuildPlan, Op, PlanRun, Severity};
use crate::{VoxelTools, VoxelWorld};

/// Outcome of a voice intent.
#[derive(Debug, Clone, PartialEq)]
pub enum VoiceRoute {
    /// Voice mode is off; the event was not ours to act on.
    Ignored,
    /// Handled entirely in-session.
    Done(Reply),
    /// A build request: run the model round-trip with this prose, then
    /// call `receive_plan_for` as for a typed `/build`.
    NeedsModel(String),
}

/// What the game should render/announce after a turn.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    /// Show this text in the chat log.
    Say(String),
    /// Show a plan preview: the echo text plus the dry-run block count.
    Preview { echo: String, blocks: usize, feature_id: String },
    /// Blocks were committed; batches are for the mesher/animator.
    Committed { blocks: usize, batches: usize, edit_ids: Vec<u64> },
    /// Nothing to do.
    Nothing,
}

#[derive(Debug, Clone)]
struct Pending {
    plan: BuildPlan,
    feature_id: String,
    predicted_blocks: usize,
}

/// What was built last, as the referent for "make it taller", "add a
/// door", "make it look old". Without this the model has nothing to
/// attach a follow-up to and rebuilds from scratch beside it.
#[derive(Debug, Clone)]
pub struct LastBuild {
    pub name: String,
    pub min: (i32, i32, i32),
    pub max: (i32, i32, i32),
    pub blocks: usize,
    /// The palette the model was told to use, if any, so a detail pass
    /// can match it.
    pub palette_hint: Option<String>,
}

#[derive(Debug, Default)]
pub struct ChatSession {
    pending: Option<Pending>,
    /// Edit ids of the last committed plan, within one VoxelTools.
    last_commit: Vec<u64>,
    /// The inverse of the last commit: (x, y, z, previous block).
    ///
    /// This exists because the edit log lives on `VoxelTools`, and a
    /// game creates fresh tools for every dispatch -- so by the time
    /// `/undo` arrives, the ids in `last_commit` refer to a log that no
    /// longer exists and undo silently restores nothing. Keeping the
    /// block values on the SESSION makes undo independent of tool
    /// lifetime. Found by running a two-turn session, not by reading.
    last_inverse: Vec<(i32, i32, i32, u16)>,
    /// The referent for follow-up turns.
    pub last_build: Option<LastBuild>,
    /// Batches and ops from the last commit, for the animator. The
    /// reveal must be scheduled from what was actually written, not
    /// from re-running the plan: a seeded scatter re-run changes
    /// nothing and would schedule no decoration at all.
    last_batches: Vec<Batch>,
    last_ops: Vec<Op>,
    last_anchor: (i32, i32, i32),
    /// Voice live mode: spoken build requests preview immediately and
    /// "build it" commits. Off by default; `/build enableVoice`.
    pub voice_live: bool,
    /// Reveal profile for commits. `/anim <profile>`.
    pub profile: Profile,
    tier_override: Option<ScaleTier>,
    /// Block budget for check_math's scale warning.
    pub budget: i64,
    pub chunk_size: i32,
    pub from_y: i32,
}

impl ChatSession {
    pub fn new(chunk_size: i32, from_y: i32) -> Self {
        Self { budget: 200_000, chunk_size, from_y, profile: Profile::GroundUp, ..Default::default() }
    }

    /// `/build enableVoice` / `disableVoice`. Returns the grammar the
    /// game must register with voiced when turning on (empty when
    /// turning off), so the caller has everything in one place.
    pub fn set_voice(&mut self, on: bool) -> (Reply, Vec<Vec<String>>) {
        self.voice_live = on;
        if on {
            (Reply::Say("voice on: say \"build ...\" to preview, \"build it\" to commit, \"undo\" to reverse".into()),
             crate::voice::grammar("unendlich"))
        } else {
            (Reply::Say("voice off".into()), Vec::new())
        }
    }

    /// `/anim <profile>`.
    pub fn set_profile(&mut self, p: Profile) -> Reply {
        self.profile = p;
        Reply::Say(format!("builds will reveal {}", p.name()))
    }

    /// A recognised utterance. Voice is a second keyboard: everything
    /// routes through the same paths as typed commands, except Cancel
    /// (drop the pending plan) which has no chat verb.
    pub fn voice<W: VoxelWorld>(&mut self, tools: &mut VoxelTools<W>, gaze: &Gaze,
                                intent: &VoiceIntent) -> VoiceRoute {
        if !self.voice_live { return VoiceRoute::Ignored; }
        match intent {
            VoiceIntent::Cancel => { self.pending = None; VoiceRoute::Done(Reply::Say("cancelled".into())) }
            VoiceIntent::Commit => VoiceRoute::Done(self.commit(tools, gaze)),
            VoiceIntent::Undo => VoiceRoute::Done(self.undo(tools)),
            VoiceIntent::Bigger => VoiceRoute::Done(self.adjust_focus(gaze, Some(true))),
            VoiceIntent::Smaller => VoiceRoute::Done(self.adjust_focus(gaze, Some(false))),
            VoiceIntent::ShowPlan => VoiceRoute::Done(self.show_plan()),
            // a build request needs the model: hand back the prose so
            // the caller runs the round-trip exactly as for /build
            VoiceIntent::Build(req) => VoiceRoute::NeedsModel(req.clone()),
        }
    }

    /// Current focus for a gaze, honouring any /focus override.
    pub fn focus(&self, gaze: &Gaze) -> Focus {
        Focus::from_gaze_at_tier(gaze, self.tier_override)
    }

    /// The context block to prepend to the model prompt. Includes the
    /// region digest so the model has actually "read the file".
    pub fn context<W: VoxelWorld>(&self, tools: &VoxelTools<W>, gaze: &Gaze,
                                  surface_name: Option<&str>) -> String {
        let f = self.focus(gaze);
        let (mn, mx) = f.region.bounds();
        let digest = tools.read_region(mn, mx);
        let col = tools.probe_column(f.anchor.0, f.anchor.2, self.from_y, 6);
        let last = match &self.last_build {
            Some(lb) => format!(
                "\nLAST BUILD '{}': box {:?}..{:?}, {} blocks{}. Follow-ups should attach to it; \
                 @0,0,0 is the focus anchor, not this box -- use its coordinates.",
                lb.name, lb.min, lb.max, lb.blocks,
                lb.palette_hint.as_ref().map(|p| format!(", palette {p}")).unwrap_or_default()),
            None => String::new(),
        };
        format!("{}\n{digest}\n{col}{last}", f.read_hint(surface_name))
    }

    /// Does the focus currently sit on or inside the last build? Used
    /// to decide whether an unspecified request is a detail pass on
    /// the existing thing or a fresh massing pass beside it.
    pub fn focus_on_last_build(&self, gaze: &Gaze) -> bool {
        let Some(lb) = &self.last_build else { return false };
        let a = self.focus(gaze).anchor;
        let pad = 3;
        a.0 >= lb.min.0 - pad && a.0 <= lb.max.0 + pad
            && a.2 >= lb.min.2 - pad && a.2 <= lb.max.2 + pad
    }

    /// The full user turn for the model: context, inferred pass, prose.
    /// The pass can be forced ("expand" from a /build --expand flag or a
    /// UI button); otherwise it is inferred from the words and from
    /// whether the player is looking at their last build.
    pub fn user_turn<W: VoxelWorld>(&self, tools: &VoxelTools<W>, gaze: &Gaze,
                                    surface_name: Option<&str>, prose: &str,
                                    force_pass: Option<Pass>) -> (String, Pass) {
        let pass = force_pass.unwrap_or_else(|| Pass::infer(prose, self.focus_on_last_build(gaze)));
        let ctx = self.context(tools, gaze, surface_name);
        (user_turn(&ctx, pass, prose), pass)
    }

    /// `/focus bigger|smaller|` -- adjust or report the tier.
    pub fn adjust_focus(&mut self, gaze: &Gaze, bigger: Option<bool>) -> Reply {
        let cur = self.focus(gaze);
        match bigger {
            None => Reply::Say(format!("focus {}: tier {} anchored at {:?}",
                                       cur.feature_id, cur.tier.name(), cur.anchor)),
            Some(true) => {
                self.tier_override = Some(cur.tier.expand());
                let f = self.focus(gaze);
                Reply::Say(format!("focus expanded to {} (read radius {})",
                                   f.tier.name(), f.tier.read_radius()))
            }
            Some(false) => {
                let t = match cur.tier {
                    ScaleTier::Horizon => ScaleTier::Landscape,
                    ScaleTier::Landscape => ScaleTier::Structure,
                    _ => ScaleTier::Detail,
                };
                self.tier_override = Some(t);
                Reply::Say(format!("focus narrowed to {} (read radius {})", t.name(), t.read_radius()))
            }
        }
    }

    /// The model has answered with line IR. Parse, validate, dry-run,
    /// and hold it pending. `ir` is the model's reply text.
    pub fn receive_plan<W: VoxelWorld>(&mut self, tools: &mut VoxelTools<W>,
                                       gaze: &Gaze, ir: &str) -> Reply {
        self.receive_plan_for(tools, gaze, ir, None)
    }

    /// As `receive_plan`, with the player's feet position so the preview
    /// can warn about entombment and floating blocks.
    pub fn receive_plan_for<W: VoxelWorld>(&mut self, tools: &mut VoxelTools<W>,
                                           gaze: &Gaze, ir: &str,
                                           player_feet: Option<(i32, i32, i32)>) -> Reply {
        let f = self.focus(gaze);
        let (plan, mut issues) = parse_plan_at(ir, Some(f.anchor));
        issues.extend(validate(&plan));
        let errors: Vec<_> = issues.iter().filter(|i| i.severity == Severity::Error).collect();
        if !errors.is_empty() {
            let msg = errors.iter().map(|i| format!("op {}: {}", i.op, i.message))
                .collect::<Vec<_>>().join("\n");
            self.pending = None;
            return Reply::Say(format!("plan rejected:\n{msg}"));
        }
        let math = check_math(&plan, self.budget);
        if math.iter().any(|n| n.level == Note::Error) {
            self.pending = None;
            let msg = math.iter().filter(|n| n.level == Note::Error)
                .map(|n| n.message.clone()).collect::<Vec<_>>().join("\n");
            return Reply::Say(format!("plan rejected (arithmetic):\n{msg}"));
        }
        // Dry run, but inspect the placed set BEFORE it is reversed:
        // the critique needs the blocks in the world to test support.
        let first = tools.edits.len();
        let live: PlanRun = run_plan(tools, &plan, self.from_y, self.chunk_size, false);
        let placed: Vec<(i32, i32, i32)> = tools.edits[first..].iter()
            .flat_map(|e| e.before.iter().map(|&(x, y, z, _)| (x, y, z))).collect();
        let mut findings = critique::floating(tools.world, &placed);
        if let Some(feet) = player_feet {
            if let Some(f) = critique::entombs_player(&placed, feet) { findings.push(f); }
        }
        // now reverse, exactly as a dry run would
        let ids: Vec<u64> = tools.edits[first..].iter().map(|e| e.id).collect();
        for id in ids.into_iter().rev() { tools.undo_edit(id); }
        let dry = live;
        let mut echo = explain_plan(&plan, self.budget);
        if !findings.is_empty() {
            echo.push_str(&format!("\nCHECK  {}\n", critique::summarise(&findings)));
        }
        if dry.failed > 0 {
            echo.push_str(&format!("\n{} op(s) failed in preview:\n", dry.failed));
            for l in dry.log.iter().filter(|l| l.contains("refused") || l.contains("no ")) {
                echo.push_str(&format!("  {l}\n"));
            }
        }
        for w in issues.iter().filter(|i| i.severity == Severity::Warning) {
            echo.push_str(&format!("\nwarn op {}: {}", w.op, w.message));
        }
        self.pending = Some(Pending {
            plan, feature_id: f.feature_id.clone(), predicted_blocks: dry.blocks_changed,
        });
        Reply::Preview { echo, blocks: dry.blocks_changed, feature_id: f.feature_id }
    }

    /// Drop any pending plan.
    ///
    /// Called when a new build TURN STARTS, not when it succeeds. If a
    /// turn fails -- provider missing, model unreachable, reply
    /// unparseable -- the previous turn's plan was still sitting in
    /// `pending`, so the next `/commit` silently rebuilt the OLD thing.
    /// Reported from the field as "/commit just put another tower
    /// instead of waiting for the model": it was not waiting, it was
    /// committing the stale fixture plan.
    pub fn clear_pending(&mut self) -> bool {
        self.pending.take().is_some()
    }

    /// `/plan` -- re-show the pending preview.
    pub fn show_plan(&self) -> Reply {
        match &self.pending {
            Some(p) => Reply::Preview {
                echo: explain_plan(&p.plan, self.budget),
                blocks: p.predicted_blocks,
                feature_id: p.feature_id.clone(),
            },
            None => Reply::Say("no pending plan; /build something first".into()),
        }
    }

    /// `/commit` -- apply the pending plan, if the focus it was made
    /// for is still the focus.
    pub fn commit<W: VoxelWorld>(&mut self, tools: &mut VoxelTools<W>, gaze: &Gaze) -> Reply {
        let Some(p) = self.pending.take() else {
            return Reply::Say("nothing to commit".into());
        };
        let now = self.focus(gaze);
        if now.feature_id != p.feature_id {
            // Refuse rather than build somewhere else. The plan's
            // @coordinates were resolved against the old anchor at
            // parse time, so committing it here would place the build
            // where the player WAS looking, which is surprising in the
            // worst way.
            self.pending = Some(p);
            return Reply::Say(format!(
                "focus changed since the plan was made ({} -> {}); look back at the spot, \
                 or /build again from here", now.tier.name(), now.tier.name()));
        }
        let first = tools.edits.len();
        let run = run_plan(tools, &p.plan, self.from_y, self.chunk_size, false);
        self.last_commit = tools.edits[first..].iter().map(|e| e.id).collect();
        // Remember the box for follow-ups. Merge with the previous last
        // build when they overlap, so "taller" then "add a door" keeps
        // one growing referent rather than replacing it each turn.
        let (mut mn, mut mx) = ((i32::MAX, i32::MAX, i32::MAX), (i32::MIN, i32::MIN, i32::MIN));
        for e in &tools.edits[first..] {
            for &(x, y, z, _) in &e.before {
                mn = (mn.0.min(x), mn.1.min(y), mn.2.min(z));
                mx = (mx.0.max(x), mx.1.max(y), mx.2.max(z));
            }
        }
        if run.blocks_changed > 0 {
            let touches_prev = self.last_build.as_ref().map(|lb| {
                mn.0 <= lb.max.0 + 2 && mx.0 >= lb.min.0 - 2
                    && mn.2 <= lb.max.2 + 2 && mx.2 >= lb.min.2 - 2
            }).unwrap_or(false);
            let (mn, mx, blocks, name) = if touches_prev {
                let lb = self.last_build.as_ref().unwrap();
                // keep the referent's name: "watchtower" should not
                // become "build" because a follow-up plan was unnamed
                let name = if p.plan.name == "build" { lb.name.clone() } else { p.plan.name.clone() };
                ((mn.0.min(lb.min.0), mn.1.min(lb.min.1), mn.2.min(lb.min.2)),
                 (mx.0.max(lb.max.0), mx.1.max(lb.max.1), mx.2.max(lb.max.2)),
                 lb.blocks + run.blocks_changed, name)
            } else { (mn, mx, run.blocks_changed, p.plan.name.clone()) };
            self.last_build = Some(LastBuild { name, min: mn, max: mx, blocks, palette_hint: None });
        }
        self.last_ops = p.plan.ops.clone();
        self.last_batches = run.batches.clone();
        // Capture the inverse now, newest-first, while the log is alive.
        self.last_inverse = tools.edits[first..].iter().rev()
            .flat_map(|e| e.before.iter().rev().copied())
            .map(|(x, y, z, prev)| (x, y, z, prev))
            .collect();
        self.last_anchor = self.focus(gaze).anchor;
        Reply::Committed { blocks: run.blocks_changed, batches: run.batches.len(),
                           edit_ids: self.last_commit.clone() }
    }

    /// What the last commit wrote, for `anim::schedule`.
    pub fn last_commit_batches(&self) -> (&[Op], &[Batch]) {
        (&self.last_ops, &self.last_batches)
    }

    /// The reveal for the last commit, under the current profile.
    pub fn reveal(&self, blocks_per_frame: usize) -> Vec<crate::anim::Frame> {
        crate::anim::schedule_with(self.profile, &self.last_ops, &self.last_batches,
                                   self.last_anchor, blocks_per_frame, self.chunk_size, 7)
    }

    /// `/undo` -- reverse the last committed plan as a unit.
    pub fn undo<W: VoxelWorld>(&mut self, tools: &mut VoxelTools<W>) -> Reply {
        if self.last_inverse.is_empty() {
            return Reply::Say("nothing to undo".into());
        }
        let inv = std::mem::take(&mut self.last_inverse);
        let ops = self.last_ops.len().max(1);
        let mut n = 0usize;
        // Apply the recorded previous values directly. Newest-first, so
        // a cell edited twice in one commit ends at its original value.
        for &(x, y, z, prev) in &inv {
            if tools.world.set(x, y, z, prev) { n += 1; }
        }
        self.last_commit.clear();
        self.last_build = None;
        self.last_batches.clear();
        Reply::Say(format!("undid last build: {n} blocks restored across {ops} ops"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::focus::GazeHit;
    use std::collections::BTreeMap;

    struct Flat { cells: BTreeMap<(i32, i32, i32), u16> }
    impl Flat { fn new() -> Self { Self { cells: BTreeMap::new() } } }
    impl VoxelWorld for Flat {
        fn get(&self, x: i32, y: i32, z: i32) -> u16 {
            if let Some(&b) = self.cells.get(&(x, y, z)) { return b; }
            if y <= 64 { 1 } else { 0 }
        }
        fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.cells.insert((x, y, z), b); true }
    }
    fn gaze_at(x: i32, z: i32, dist: f32) -> Gaze {
        Gaze { eye: (0.0, 70.0, 0.0), dir: (1.0, -0.3, 0.0),
               hit: Some(GazeHit { block: (x, 64, z), face_normal: (0, 1, 0), distance: dist }) }
    }

    #[test]
    fn context_tells_the_model_to_read_before_writing() {
        let mut w = Flat::new();
        let t = VoxelTools::new(&mut w);
        let s = ChatSession::new(32, 200);
        let ctx = s.context(&t, &gaze_at(20, 5, 15.0), Some("grass"));
        assert!(ctx.contains("READ FIRST"));
        assert!(ctx.contains("% solid"), "region digest included: {ctx}");
        assert!(ctx.contains("column (20,5)"), "probe at the anchor: {ctx}");
    }

    #[test]
    fn plan_relative_to_focus_previews_then_commits() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        let g = gaze_at(20, 5, 15.0);
        let r = s.receive_plan(&mut t, &g, "tower @0,0 r=3 h=6 block=stone\n");
        let (blocks, fid) = match r {
            Reply::Preview { blocks, feature_id, .. } => (blocks, feature_id),
            other => panic!("expected preview, got {other:?}"),
        };
        assert!(blocks > 0);
        // preview must not have built anything
        assert_eq!(t.world.get(23, 66, 5), 0, "dry run left the world alone");
        let c = s.commit(&mut t, &g);
        match c {
            Reply::Committed { blocks: b, .. } => assert_eq!(b, blocks, "commit matches preview"),
            other => panic!("{other:?}"),
        }
        assert_ne!(t.world.get(23, 66, 5), 0, "tower wall exists at anchor+3 after commit");
        assert!(!fid.is_empty());
    }

    #[test]
    fn commit_refuses_when_focus_moved_away() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        s.receive_plan(&mut t, &gaze_at(20, 5, 15.0), "tower @0,0 r=3 h=6\n");
        // player now looks far away: different tier, different feature id
        let r = s.commit(&mut t, &gaze_at(300, 300, 80.0));
        match r { Reply::Say(m) => assert!(m.contains("focus changed"), "{m}"), o => panic!("{o:?}") }
        assert_eq!(t.world.get(23, 66, 5), 0, "nothing built in the wrong place");
        assert!(s.show_plan() != Reply::Say("no pending plan; /build something first".into()),
                "the plan stays pending for when they look back");
    }

    #[test]
    fn rejected_plans_say_why_and_leave_nothing_pending() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        let r = s.receive_plan(&mut t, &gaze_at(0, 0, 10.0), "# empty\n");
        match r { Reply::Say(m) => assert!(m.contains("rejected"), "{m}"), o => panic!("{o:?}") }
        assert_eq!(s.show_plan(), Reply::Say("no pending plan; /build something first".into()));
    }

    #[test]
    fn undo_reverses_the_whole_last_build() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        let g = gaze_at(0, 0, 10.0);
        s.receive_plan(&mut t, &g, "tower @0,0 r=2 h=4\ntree @8,0 oak h=6\n");
        s.commit(&mut t, &g);
        assert_ne!(t.world.get(2, 66, 0), 0);
        let r = s.undo(&mut t);
        match r { Reply::Say(m) => assert!(m.contains("2 ops"), "{m}"), o => panic!("{o:?}") }
        assert_eq!(t.world.get(2, 66, 0), 0, "tower gone");
        assert_eq!(t.world.get(8, 66, 0), 0, "tree gone");
    }

    #[test]
    fn focus_bigger_changes_the_read_radius() {
        let mut s = ChatSession::new(32, 200);
        let g = gaze_at(0, 0, 3.0);
        assert_eq!(s.focus(&g).tier, ScaleTier::Detail);
        s.adjust_focus(&g, Some(true));
        assert_eq!(s.focus(&g).tier, ScaleTier::Structure);
        assert_eq!(s.focus(&g).tier.read_radius(), 16);
    }

    #[test]
    fn last_build_becomes_the_referent_for_follow_ups() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        let g = gaze_at(0, 0, 10.0);
        s.receive_plan(&mut t, &g, "# tower\ntower @0,0 r=3 h=8\n");
        s.commit(&mut t, &g);
        let lb = s.last_build.clone().expect("recorded");
        assert_eq!(lb.name, "tower");
        assert!(lb.blocks > 0);
        // the context now names it, so the model can attach to it
        let ctx = s.context(&t, &g, Some("grass"));
        assert!(ctx.contains("LAST BUILD 'tower'"), "{ctx}");
        // and an unspecified request while looking at it is a detail pass
        let (_, pass) = s.user_turn(&t, &g, Some("grass"), "add a door", None);
        assert_eq!(pass, Pass::Detail);
        // looking elsewhere: it is a fresh massing pass
        let (_, pass2) = s.user_turn(&t, &gaze_at(80, 80, 10.0), Some("grass"), "a hut", None);
        assert_eq!(pass2, Pass::Massing);
    }

    #[test]
    fn overlapping_builds_merge_into_one_growing_referent() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        let g = gaze_at(0, 0, 10.0);
        s.receive_plan(&mut t, &g, "tower @0,0 r=3 h=6\n");
        s.commit(&mut t, &g);
        let first = s.last_build.clone().unwrap();
        // "taller": a second cylinder on top, overlapping in x/z
        s.receive_plan(&mut t, &g, "cylinder @0,0 r=3 h=5 y=72\n");
        s.commit(&mut t, &g);
        let merged = s.last_build.clone().unwrap();
        assert!(merged.max.1 > first.max.1, "the referent grew upward");
        assert!(merged.blocks > first.blocks);
    }

    #[test]
    fn preview_warns_about_a_floating_build_and_entombment() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        let g = gaze_at(0, 0, 10.0);
        // a slab pinned in midair with y=, and the player standing at the anchor
        // tower first (on real ground, through the player), THEN a slab in
        // midair. Ordered this way on purpose: if the slab went first,
        // find_surface would put the tower on top of it -- which is
        // exactly the failure the component-level check now catches.
        let r = s.receive_plan_for(&mut t, &g, "cylinder @0,0 r=1 h=3\nfill @-1,20,-1 @1,20,1 block=stone\n",
                                   Some((0, 65, 0)));
        match r {
            Reply::Preview { echo, .. } => {
                assert!(echo.contains("floating"), "{echo}");
                assert!(echo.contains("entombs_player"), "{echo}");
            }
            o => panic!("{o:?}"),
        }
        // and the preview still left the world untouched
        assert_eq!(t.world.get(0, 85, 0), 0);
    }

    #[test]
    fn voice_routes_through_the_same_paths_as_typing() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        let g = gaze_at(0, 0, 10.0);
        // off by default: events are ignored
        assert_eq!(s.voice(&mut t, &g, &VoiceIntent::Commit), VoiceRoute::Ignored);
        let (_, grammar) = s.set_voice(true);
        assert!(!grammar.is_empty(), "turning on hands back the grammar to register");
        // a spoken build needs the model
        assert_eq!(s.voice(&mut t, &g, &VoiceIntent::Build("a hut".into())),
                   VoiceRoute::NeedsModel("a hut".into()));
        // the model replied; preview lands as usual
        s.receive_plan(&mut t, &g, "tower @0,0 r=2 h=5\n");
        // "build it" commits through the same guarded path
        match s.voice(&mut t, &g, &VoiceIntent::Commit) {
            VoiceRoute::Done(Reply::Committed { blocks, .. }) => assert!(blocks > 0),
            o => panic!("{o:?}"),
        }
        assert_ne!(t.world.get(2, 66, 0), 0);
        // "undo" reverses it
        match s.voice(&mut t, &g, &VoiceIntent::Undo) {
            VoiceRoute::Done(Reply::Say(m)) => assert!(m.contains("undid"), "{m}"),
            o => panic!("{o:?}"),
        }
        assert_eq!(t.world.get(2, 66, 0), 0);
    }

    #[test]
    fn cancel_drops_the_pending_plan() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        let g = gaze_at(0, 0, 10.0);
        s.set_voice(true);
        s.receive_plan(&mut t, &g, "tower @0,0 r=2 h=5\n");
        s.voice(&mut t, &g, &VoiceIntent::Cancel);
        assert_eq!(s.commit(&mut t, &g), Reply::Say("nothing to commit".into()));
    }

    #[test]
    fn profile_selects_the_reveal_shape() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        let g = gaze_at(0, 0, 10.0);
        s.receive_plan(&mut t, &g, "tower @0,0 r=3 h=10\n");
        s.commit(&mut t, &g);
        let ground = s.reveal(10).len();
        s.set_profile(Profile::Instant);
        assert_eq!(s.reveal(10).len(), 1);
        s.set_profile(Profile::Physics);
        let phys = s.reveal(10);
        assert_eq!(phys.len(), ground, "same block count, same frame count");
        assert!(phys.iter().all(|f| f.drop_from.is_some()));
    }

    #[test]
    fn undo_works_with_fresh_tools_each_turn() {
        // The game builds new VoxelTools per dispatch. Undo must not
        // depend on the previous tools' edit log still existing.
        let mut w = Flat::new();
        let g = gaze_at(0, 0, 10.0);
        let mut s = ChatSession::new(32, 200);
        {
            let mut t = VoxelTools::new(&mut w);
            s.receive_plan(&mut t, &g, "tower @0,0 r=2 h=5 block=stone_bricks\n");
            s.commit(&mut t, &g);
        }
        assert_ne!(w.get(2, 66, 0), 0, "built");
        // fresh tools, as a later frame would have
        let mut t2 = VoxelTools::new(&mut w);
        match s.undo(&mut t2) {
            Reply::Say(m) => assert!(m.contains("restored") && !m.contains("0 blocks"), "{m}"),
            o => panic!("{o:?}"),
        }
        drop(t2);
        assert_eq!(w.get(2, 66, 0), 0, "undo restored the world through new tools");
    }

    #[test]
    fn a_failed_turn_leaves_nothing_to_commit() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        let g = gaze_at(0, 0, 10.0);
        // turn 1 succeeds and is committed
        s.receive_plan(&mut t, &g, "tower @0,0 r=2 h=5 block=stone_bricks\n");
        s.commit(&mut t, &g);
        // turn 2 starts: clear first, then the provider fails, so no plan
        assert!(s.clear_pending() == false, "nothing pending after a commit");
        s.receive_plan(&mut t, &g, "tower @9,9 r=2 h=5\n");
        assert!(s.clear_pending(), "a fresh preview is pending");
        // with it cleared, a commit must not resurrect the old plan
        assert_eq!(s.commit(&mut t, &g), Reply::Say("nothing to commit".into()));
    }
}
