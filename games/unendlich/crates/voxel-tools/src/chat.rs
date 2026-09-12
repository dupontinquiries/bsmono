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

/// Where a pending plan will land.
#[derive(Debug, Clone, PartialEq)]
pub struct Waypoint {
    pub anchor: (i32, i32, i32),
    pub name: String,
    pub blocks: usize,
    /// Bounding box of the dry run, if it touched anything.
    pub bounds: Option<((i32, i32, i32), (i32, i32, i32))>,
}

#[derive(Debug, Clone)]
pub struct Pending {
    plan: BuildPlan,
    feature_id: String,
    predicted_blocks: usize,
    /// Box of everything the dry run touched, for the waypoint marker.
    bounds: Option<((i32, i32, i32), (i32, i32, i32))>,
    /// The expanded IR and the prose that produced it.
    ir: String,
    prose: String,
    /// What the dry run placed, for a review's measurement and picture.
    placed: Vec<(i32, i32, i32)>,
    /// The program source, when this pending came from one. A review
    /// needs it: a program's ops list is empty by construction.
    program: String,
    /// Per pass: name, polarity word, cells moved.
    pass_log: Vec<(String, String, usize)>,
    /// Cells from a composed sketch that no IR op can express -- a
    /// torus, a helix. Applied alongside the plan on commit, so the
    /// whole composition is one edit and undoes as one.
    extra_cells: Vec<crate::weather::Change>,
    /// The critique's findings, in words, for the review.
    findings: Vec<String>,
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

#[derive(Debug, Default, Clone)]
pub struct ChatSession {
    /// Every attempt this build has made, so a review can show the
    /// model whether its last edit HELPED. Without it each round shows
    /// the current shape and nothing else, and a model editing blind
    /// repeats itself and undoes its own improvements.
    pub bench: crate::bench::Bench,
    /// Answers to whatever the model asked the world last turn, kept
    /// so a second attempt has them. A model that asks "what does
    /// water do" and is not told is no better off than one that never
    /// asked.
    pub probe_answers: Vec<String>,
    /// Set while a composed sketch or program is being built: the
    /// shape is decided and the recognizer must not add another.
    pub suppress_recognizer: bool,
    /// What the study ladder learned, if it was climbed. Carried to
    /// the REVIEW, so a verdict is judged against what the figure was
    /// supposed to read as rather than against nothing.
    pub study_note: Option<String>,
    /// Climate at the build site, supplied by the engine each turn.
    /// Defaults to temperate; a game that never sets it behaves exactly
    /// as before.
    pub climate: (f32, f32),
    pub sea_level: i32,
    /// Run an OUTLINE pass before the worker pass. Two round trips
    /// instead of one: worth it for a village, wasteful for a tower,
    /// so it is opt-in.
    pub two_pass: bool,
    /// How far the terrain read looks. Bigger is more context and more
    /// tokens; 24 covers a village-sized decision.
    pub scene_radius: i32,
    /// Public so a drone-produced session can hand it back to the live one.
    pub pending: Option<Pending>,
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
    /// The last context built, keyed by focus feature id and climate.
    /// A RefCell because `context` is `&self` and callers hold it that
    /// way; the cache is an implementation detail, not state.
    pub context_cache: std::cell::RefCell<Option<(String, String)>>,
    /// The IR and prose of the last committed plan, for the journal
    /// and the world tree. A commit's payload is its IR; without this
    /// the tree would have blocks and no way to rebase them.
    pub last_ir: String,
    pub last_prose: String,
    /// Sites a committed lazy scatter handed over, awaiting the engine.
    pub held_sites: Vec<(String, crate::scatter::Site)>,
    /// Living plants placed by commits, awaiting the engine.
    pub planted: Vec<crate::flora::PlantRequest>,
    /// Builds the current prose names, resolved by the host from the
    /// claims before the turn. Cleared by the host after.
    pub referents: Vec<LastBuild>,
    /// A spec line from `/like`, prepended to the next plan received so
    /// the tuner targets it. Consumed by that plan.
    pub pending_spec: Option<String>,
    /// The last plan's fatal error, if it had one: what to show on the
    /// marker when a round fails.
    pub last_plan_error: Option<String>,
    /// Elite decorator genomes for the session: a second gothic wall
    /// starts where the first finished. Lent to the tools around every
    /// run and taken back after; the host copies it back from a drone.
    pub archive: crate::evolve::Archive<crate::gene::Genome>,
    /// Training records from spec tuning: prose, spec, before, after.
    /// The host appends them to the world's training file.
    pub training: Vec<String>,
    /// A per-turn creative seed: the same request twice should not give
    /// the same building. Advanced by every plan received, so "another
    /// one" varies; a stated `seed=` in the IR still wins.
    pub creative_seed: u64,
    /// Seed for the refinement ladder, so re-running a rung reproduces
    /// it and running the next one does not repeat the last one's noise.
    pub refine_seed: i32,
    /// Lines the model got wrong on the last turn, for `/repair`.
    pub last_salvage: Option<crate::recovery::Salvage>,
    /// Blocks that could not be placed because their chunk was not
    /// loaded, waiting for a retry.
    pub deferred: crate::recovery::Deferred,
    /// Batches and ops from the last commit, for the animator. The
    /// reveal must be scheduled from what was actually written, not
    /// from re-running the plan: a seeded scatter re-run changes
    /// nothing and would schedule no decoration at all.
    last_batches: Vec<Batch>,
    last_ops: Vec<Op>,
    /// The anchor the last committed plan's `@` coordinates were
    /// resolved against. The PLAN's anchor, not the gaze at commit: a
    /// commit may now come from anywhere, and both the reveal ordering
    /// and a rebase replay against this.
    pub last_anchor: (i32, i32, i32),
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
        Self { budget: 200_000, chunk_size, from_y, profile: Profile::GroundUp,
               climate: (0.5, 0.5), sea_level: 64, scene_radius: 24, two_pass: false,
               refine_seed: 1, creative_seed: 1, last_plan_error: None,
               ..Default::default() }
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
        // The terrain read, the ambient reading and the region digest
        // are a few thousand surface probes -- on the frame thread, in
        // `prompt_for`. Cached by focus, climate, surface and referent.
        //
        // The feature id encodes gaze and tier, NOT what is under them:
        // it was the same before and after a build, and the first
        // version of this cache served the pre-build terrain to the
        // next turn. So the cache is also INVALIDATED explicitly by
        // commit, undo and the host whenever the world changes.
        let lb = self.last_build.as_ref().map(|b| format!("{}{:?}{:?}", b.name, b.min, b.max))
            .unwrap_or_default()
            + &self.referents.iter().map(|r| format!("|{}{:?}", r.name, r.min)).collect::<String>();
        let key = format!("{}|{:.2}|{:.2}|{}|{}", f.feature_id, self.climate.0, self.climate.1,
                          surface_name.unwrap_or(""), lb);
        if let Some((k, text)) = &*self.context_cache.borrow() {
            if *k == key { return text.clone(); }
        }
        let text = self.context_uncached(tools, gaze, surface_name);
        *self.context_cache.borrow_mut() = Some((key, text.clone()));
        text
    }

    fn context_uncached<W: VoxelWorld>(&self, tools: &VoxelTools<W>, gaze: &Gaze,
                                       surface_name: Option<&str>) -> String {
        let f = self.focus(gaze);
        let (mn, mx) = f.region.bounds();
        let digest = tools.read_region(mn, mx);
        let col = tools.probe_column(f.anchor.0, f.anchor.2, self.from_y, 6);
        // What the surroundings say belongs here. One line: it competes
        // for context with the region digest, and a small model drowns.
        let amb = tools.read_ambient(f.anchor.0, f.anchor.2, self.from_y,
                                     self.climate.0, self.climate.1, self.sea_level);
        // Positioned: the rock underfoot varies across the world, and
        // what a builder here would quarry is what the model should
        // reach for.
        let site = amb.prompt_line_at(f.anchor.0, f.anchor.2, self.creative_seed);
        // Perception is COMPUTED, not asked for. A scene-setting model
        // pass would spend a third of the latency producing a worse
        // description than the one the world can just be read for.
        let scene = crate::scene::read(tools, f.anchor, self.scene_radius,
                                       self.from_y, 15);
        let mut last = match &self.last_build {
            Some(lb) => format!(
                "\nLAST BUILD '{}': box {:?}..{:?}, {} blocks{}. Follow-ups should attach to it; \
                 @0,0,0 is the focus anchor, not this box -- use its coordinates.",
                lb.name, lb.min, lb.max, lb.blocks,
                lb.palette_hint.as_ref().map(|p| format!(", palette {p}")).unwrap_or_default()),
            None => String::new(),
        };
        // NAMED BUILDS the prose mentions, resolved by the host from the
        // claims: "make the mill taller" gets the mill's box even when
        // the last build was the well. Without this a name the player
        // uses is a name the model cannot see.
        for r in &self.referents {
            last.push_str(&format!(
                "\nNAMED BUILD '{}': box {:?}..{:?}, {} blocks. The player refers to it by name; \
                 use its coordinates.", r.name, r.min, r.max, r.blocks));
        }
        format!("{}\n{site}\n\n{}\n\n{digest}\n{col}{last}",
                f.read_hint(surface_name), scene.prompt_block())
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

    /// The refinement ladder for the last build, read from the site.
    pub fn refine_ladder<W: VoxelWorld>(&self, tools: &VoxelTools<W>, gaze: &Gaze)
        -> Option<Vec<crate::refine::Rung>> {
        let lb = self.last_build.as_ref()?;
        let amb = tools.read_ambient(lb.min.0, lb.min.2, self.from_y,
                                     self.climate.0, self.climate.1, self.sea_level);
        let ws = crate::land::Workset {
            id: "last".into(), name: lb.name.clone(), owner: String::new(),
            members: vec![], min: lb.min, max: lb.max, turns: vec![] };
        let _ = gaze;
        Some(crate::refine::ladder(&ws, &amb, self.refine_seed))
    }

    /// A macro the prose names, expanded against the last build, so it
    /// needs no model at all. "Fortify my base" is geometry once the
    /// base's box is known; asking a 1B to derive nine coordinated
    /// lines for it is asking for one of them to be wrong.
    ///
    /// Returns the IR to feed straight into `receive_plan_for`, or None
    /// when the prose is not a macro or there is nothing to apply it to.
    pub fn macro_ir(&self, prose: &str, gaze: &Gaze) -> Option<String> {
        let verb = crate::verbs::recognize(prose)?;
        let lb = self.last_build.as_ref()?;
        // Only when the player is actually looking at the thing: "fortify
        // my base" while staring at a field is a request for a new base,
        // and that one the model should handle.
        if !self.focus_on_last_build(gaze) { return None; }
        let ws = crate::land::Workset {
            id: "last".into(), name: lb.name.clone(), owner: String::new(),
            members: vec![], min: lb.min, max: lb.max, turns: vec![] };
        let feet = self.focus(gaze).anchor;
        Some(match verb {
            crate::verbs::Verb::Fortify =>
                crate::verbs::fortify(&ws, 3, 5, "stone", "dark_oak_planks", (feet.0, feet.2)),
            crate::verbs::Verb::Expand(side) => crate::verbs::expand(&ws, &side, 6, "stone"),
            crate::verbs::Verb::Light => crate::verbs::light(&ws, 4, "glow_crystal"),
            crate::verbs::Verb::Repair => crate::verbs::repair(&ws, "stone"),
            // Connect names two builds; the session knows only the last
            // one. The paintbrush resolves names against the land
            // registry and calls `connect_ir`.
            crate::verbs::Verb::Connect => return None,
        })
    }

    /// A composed sketch, from the four-pass pipeline: parts and
    /// relations resolved to a plan. The cells are for shapes the IR
    /// has no op for -- a torus, a helix -- which are applied directly.
    pub fn receive_sketch<W: VoxelWorld>(&mut self, tools: &mut VoxelTools<W>, gaze: &Gaze,
                                         sketch: &str, player_feet: Option<(i32, i32, i32)>,
                                         prose: &str) -> Reply {
        let f = self.focus(gaze);
        let anchor = f.anchor;
        let built = crate::compose::build(sketch, anchor);
        self.suppress_recognizer = true;
        if built.parts == 0 {
            return Reply::Say(format!("the sketch had no parts{}",
                if built.errors.is_empty() { String::new() } else { format!(" -- {}", built.errors.join("; ")) }));
        }
        // The plan first: the shapes that have ops go through the whole
        // existing pipeline -- dry run, finishing, critique,
        // reachability, hive.
        let mut r = self.receive_plan_gated(tools, gaze, &built.ir, player_feet, prose);
        self.suppress_recognizer = false;
        // Then the shapes with NO op -- a torus, a helix, a link. These
        // are cells and no IR line can say them, so they join the
        // pending placement directly. They are part of the same preview
        // and commit, which is what matters: a ring that committed
        // separately would not undo with its own building.
        if !built.cells.is_empty() {
            let changes: Vec<crate::weather::Change> = built.cells.iter()
                .map(|(x, y, z, m)| crate::weather::Change {
                    at: (*x, *y, *z), to: crate::plan::resolve_block(m).unwrap_or(1) })
                .collect();
            let n = changes.len();
            if let Some(p) = self.pending.as_mut() {
                p.placed.extend(changes.iter().map(|c| c.at));
                p.predicted_blocks += n;
                p.extra_cells = changes;
            } else {
                return Reply::Say("the sketch produced cells but no plan to carry them".into());
            }
            // The reply was built before these were added, so its count
            // was the plan's alone -- a ring reported as 0 blocks and
            // then appeared on commit, which is the kind of surprise
            // /commit must never spring.
            if let Reply::Preview { blocks, echo, .. } = &mut r {
                *blocks += n;
                echo.push_str(&format!("\nSKETCH: {n} cell(s) in shapes the IR cannot name"));
            }
        }
        // the sketch's own notes are the interesting part of the echo:
        // "'core' was too big to sit inside 'shell'; scaled to 60%"
        if let Reply::Preview { echo, .. } = &mut r {
            for n in built.notes.iter().chain(built.errors.iter()) {
                echo.push_str(&format!("\nSKETCH: {n}"));
            }
        }
        r
    }

    /// A PROGRAM: assets built once, then passes that add or cut.
    ///
    /// This is the path for anything the dictionary cannot say. Unlike
    /// `receive_sketch` it produces a cell set directly -- passes
    /// alternate polarity and the accumulation is the answer, so there
    /// is no IR to run. The result still previews, commits, undoes and
    /// journals like a plan.
    pub fn receive_program<W: VoxelWorld>(&mut self, _tools: &mut VoxelTools<W>, gaze: &Gaze,
                                          src: &str, player_feet: Option<(i32, i32, i32)>,
                                          prose: &str) -> Reply {
        let f = self.focus(gaze);
        let anchor = f.anchor;
        let (prog, errs) = crate::program::parse(src);
        if prog.passes.is_empty() {
            return Reply::Say(format!("the program had no passes{}",
                if errs.is_empty() { String::new() } else { format!(" -- {}", errs.join("; ")) }));
        }
        let run = crate::program::run(&prog, anchor);
        if run.changes.is_empty() {
            return Reply::Say(format!("the program moved nothing -- {}", run.summary()));
        }
        if run.changes.len() > crate::MAX_BLOCKS_PER_BRUSH {
            return Reply::Say(format!("refused: {} cells is past the landscape limit", run.changes.len()));
        }
        // A program is already a cell set, so it rides as a pending
        // plan with no ops: an empty plan carrying `extra_cells`. That
        // keeps every downstream thing -- preview, waypoint, commit,
        // undo, the world tree -- working unchanged.
        let placed: Vec<(i32, i32, i32)> = run.changes.iter().map(|c| c.at).collect();
        let bounds = placed.iter().fold(None::<((i32,i32,i32),(i32,i32,i32))>, |b, p| Some(match b {
            None => (*p, *p),
            Some((lo, hi)) => ((lo.0.min(p.0), lo.1.min(p.1), lo.2.min(p.2)),
                               (hi.0.max(p.0), hi.1.max(p.1), hi.2.max(p.2))),
        }));
        let mut echo = format!("Program '{}' -- {}", prose, run.summary());
        for n in run.notes.iter().chain(run.errors.iter()).chain(errs.iter()) {
            echo.push_str(&format!("\nPROGRAM: {n}"));
        }
        let cut = run.changes.iter().filter(|c| c.to == 0).count();
        echo.push_str(&format!("\n{} added, {cut} removed", run.changes.len() - cut));
        let blocks = run.changes.len();
        self.pending = Some(Pending {
            plan: BuildPlan { name: "program".into(), ops: Vec::new(), anchor: Some(anchor) },
            feature_id: f.feature_id.clone(),
            predicted_blocks: blocks,
            ir: src.to_string(),
            prose: prose.to_string(),
            placed,
            program: src.to_string(),
            pass_log: run.log.iter().map(|(n, p, c)| (n.clone(), p.word().to_string(), *c)).collect(),
            extra_cells: run.changes,
            findings: Vec::new(),
            bounds,
        });
        // every preview is an attempt on the bench, so the next review
        // can say whether it improved
        self.bench.push(prose, src, self.pending.as_ref().map(|p| p.placed.iter().copied().collect())
                        .unwrap_or_default(), Vec::new());
        self.invalidate_context();
        let _ = player_feet;
        Reply::Preview { echo, blocks, feature_id: f.feature_id }
    }

    /// Drop the pending plan without building it. A blockout is a
    /// preview to be JUDGED and thrown away, so it must never reach
    /// the world.
    pub fn cancel(&mut self) { self.pending = None; }

    /// The cells the pending plan would place. The dossier's lighting
    /// pass needs the FINISHED thing, and a build made part by part is
    /// only whole once every part has landed.
    pub fn pending_cells(&self) -> Option<Vec<(i32, i32, i32)>> {
        self.pending.as_ref().map(|p| p.placed.clone())
    }

    /// The pending plan's elevations, for a judgement made by looking.
    pub fn pending_elevation(&self) -> Option<String> {
        let p = self.pending.as_ref()?;
        let cells: std::collections::BTreeSet<(i32, i32, i32)> = p.placed.iter().copied().collect();
        Some(crate::review::elevations(&cells, 30, 16))
    }

    /// The review prompt for the pending plan: its ops by number, its
    /// spec, what it measured, two elevations, the critique's findings.
    /// None when nothing is pending. `round` is 1 for the first review.
    pub fn review_prompt(&self, round: usize, variants: &[crate::review::Variant]) -> Option<String> {
        let p = self.pending.as_ref()?;
        let cells: std::collections::BTreeSet<(i32, i32, i32)> = p.placed.iter().copied().collect();
        let ground = p.plan.anchor.map(|a| a.1 - 1).unwrap_or(63);
        let measured = crate::spec::measure(&cells, ground).describe();
        let elev = crate::review::elevations(&cells, 24, 14);
        let spec = p.plan.ops.iter().find_map(|o| match o { Op::Spec(s) => Some(s.clone()), _ => None });
        let ops: Vec<Op> = p.plan.ops.iter().filter(|o| !matches!(o, Op::Spec(_))).cloned().collect();
        // A PROGRAM has no ops, so a review of one showed the model an
        // empty plan and asked what to change about it. The most
        // expressive path was the one path with no feedback. A program
        // is reviewed in its own terms: passes, their polarity, and how
        // many cells each moved.
        if ops.is_empty() && !p.program.is_empty() {
            return Some(crate::review::program_prompt(
                &p.prose, &p.program, &p.pass_log, &measured, &elev, &p.findings, round));
        }
        let mut base = crate::review::prompt(&p.prose, &ops, spec.as_deref(), &measured, &elev,
                                            &p.findings, variants, round);
        // What the study decided this must READ as. Without it a review
        // of a figure judges a shape against nothing and says "looks
        // fine", which is how a bland statue survives every round.
        if let Some(note) = &self.study_note {
            base = format!("{note}\n{base}");
        }
        // and what changed since last round, which is the only thing
        // that answers "did my edit help"
        if self.bench.attempts.len() > 1 {
            let last = &self.bench.attempts[self.bench.attempts.len() - 1];
            let prev = &self.bench.attempts[self.bench.attempts.len() - 2];
            let d = crate::bench::compare(prev, last);
            base = format!("SINCE YOUR LAST CHANGE: {}\n{}{base}", d.summary(),
                           if self.bench.plateaued() {
                               "It has not improved for three rounds. Try something different \
                                in kind, not another adjustment.\n"
                           } else { "" });
        }
        Some(base)
    }

    /// Apply a review to the pending plan: the verdict edits the plan's
    /// IR and the result is re-planned (dry run, finishing, tuning) as a
    /// new pending. Returns the reply to show and whether anything
    /// changed; `done` or an empty verdict leaves the plan as it is.
    pub fn apply_review<W: VoxelWorld>(&mut self, tools: &mut VoxelTools<W>, gaze: &Gaze, reply: &str,
                                       player_feet: Option<(i32, i32, i32)>) -> (Reply, bool, crate::review::Review) {
        let Some(p) = self.pending.as_ref() else {
            return (Reply::Say("nothing pending to review".into()), false, Default::default());
        };
        let n_ops = p.plan.ops.iter().filter(|o| !matches!(o, Op::Spec(_))).count();
        let review = crate::review::parse(reply, n_ops);
        if review.done || review.is_empty() {
            return (Reply::Say(format!("review: {}", review.summary())), false, review);
        }
        // A PROGRAM is edited by rewriting whole passes, not lines.
        if !p.program.is_empty() {
            let revised = crate::review::apply_program(&p.program, &review);
            if revised.trim() == p.program.trim() {
                return (Reply::Say(format!("review: {} -- no change", review.summary())), false, review);
            }
            let prose = p.prose.clone();
            let r = self.receive_program(tools, gaze, &revised, player_feet, &prose);
            return (r, true, review);
        }
        let revised = crate::review::apply(&p.ir, &review);
        if revised.trim() == p.ir.trim() {
            return (Reply::Say(format!("review: {} -- no change", review.summary())), false, review);
        }
        let prose = p.prose.clone();
        let r = self.receive_plan_gated(tools, gaze, &revised, player_feet, &prose);
        (r, true, review)
    }

    /// "Something like the mill but taller": a spec from the latent
    /// space. Finds the named build in the index, nudges its point by
    /// each adjective that names an axis, and returns the spec line
    /// the tuner will target -- plus a note of what it is based on.
    pub fn like_spec(prose: &str, index: &crate::latent::Index) -> Option<(String, String)> {
        let p = prose.to_lowercase();
        // "like the mill" at the start of a command has no leading space
        let rest = if let Some(r) = p.strip_prefix("like ") { r }
                   else { let i = p.find(" like ")?; &p[i + 6..] };
        let words: Vec<&str> = rest.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
        // the referent: the first word after "like" (skipping "the", "a", "my") that names a build
        let mut it = words.iter().skip_while(|w| ["the", "a", "an", "my", "our", "that"].contains(w));
        let name = it.next()?;
        let h = index.find_named(name)?;
        let mut v = index.get(h)?;
        let mut nudges = Vec::new();
        for w in &words {
            if let Some(nv) = crate::latent::nudge(&v, w) { v = nv; nudges.push(w.to_string()); }
        }
        let spec = crate::latent::to_spec(&v);
        let mut line = String::from("spec");
        if let Some((a, b)) = spec.blocks { line.push_str(&format!(" blocks={a}-{b}")); }
        if let Some((a, b)) = spec.height { line.push_str(&format!(" height={a}-{b}")); }
        if let Some(d) = spec.durability { line.push_str(&format!(" durability={}", format!("{d:?}").to_lowercase())); }
        if let Some(s) = spec.sound { line.push_str(&format!(" sound={}", format!("{s:?}").to_lowercase())); }
        if let Some(s) = spec.shelter { line.push_str(&format!(" shelter={}", format!("{s:?}").to_lowercase())); }
        if let Some(s) = spec.symmetry { line.push_str(&format!(" symmetry={s:.2}")); }
        if let Some(o) = spec.openness { line.push_str(&format!(" openness={o:.2}")); }
        let note = format!("like '{name}'{}", if nudges.is_empty() { String::new() }
                           else { format!(", but {}", nudges.join(", ")) });
        Some((line, note))
    }

    /// "Connect A to B": a road between two named builds. The names are
    /// matched loosely against workset names -- "the mill" finds
    /// "Old Mill" -- because a player will not type an id.
    pub fn connect_ir(prose: &str, land: &crate::land::Registry) -> Result<String, String> {
        let p = prose.to_lowercase();
        let mut named: Vec<&crate::land::Workset> = land.all()
            .filter(|w| {
                let n = w.name.to_lowercase();
                !n.is_empty() && n.split_whitespace().any(|word| word.len() > 2 && p.contains(word))
            })
            .collect();
        // Order by where the name appears in the prose, so "connect the
        // mill to the barn" is mill -> barn, not barn -> mill.
        named.sort_by_key(|w| w.name.to_lowercase().split_whitespace()
            .filter_map(|word| p.find(word)).min().unwrap_or(usize::MAX));
        named.dedup_by(|a, b| a.id == b.id);
        match named.as_slice() {
            [a, b, ..] => Ok(crate::verbs::connect(a, b, 3, "cobblestone", true)),
            [_] => Err("I found one of the two builds; name both, e.g. \
                        \"connect the mill to the barn\"".into()),
            [] => Err(format!("no builds by those names; claimed here: {}",
                               land.all().map(|w| w.name.as_str()).collect::<Vec<_>>().join(", "))),
        }
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

    /// Strip world ops the player did not ask for.
    ///
    /// The second key. The model may propose a climate change; only the
    /// player's own words authorize one. A refused op is REMOVED and
    /// the rest of the plan still runs, because a good build should not
    /// be lost to one over-eager line.
    ///
    /// Asymmetric on purpose: a refusal costs one rephrase, while an
    /// unrequested change rewrites the biome the player chose to build
    /// in -- and undo restores blocks, not a climate field the engine
    /// has since spread and settled.
    fn gate_world_ops(&self, plan: &mut BuildPlan, prose: &str) -> Vec<String> {
        let mut notes = Vec::new();
        plan.ops.retain(|op| {
            let Op::World { verb, .. } = op else { return true };
            let Some(proposed) = crate::intent::WorldIntent::parse(verb) else { return false };
            match crate::intent::corroborate(prose, proposed) {
                Ok(_) => true,
                Err(r) => { notes.push(r.message); false }
            }
        });
        notes
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
        self.receive_plan_gated(tools, gaze, ir, player_feet, "")
    }

    /// As `receive_plan_for`, with the player's prose so world ops can
    /// be checked against what was actually asked for.
    pub fn receive_plan_gated<W: VoxelWorld>(&mut self, tools: &mut VoxelTools<W>,
                                             gaze: &Gaze, ir: &str,
                                             player_feet: Option<(i32, i32, i32)>,
                                             prose: &str) -> Reply {
        let f = self.focus(gaze);
        // Expand def/use FIRST, and salvage against the expanded text:
        // a `use` that becomes three lines shifts every later line
        // number, and salvage keys on them. Working on the expanded
        // source keeps numbers honest, and the repair prompt shows the
        // model concrete lines rather than a macro it has to unfold.
        // THE RECOGNIZER. If the prose names a form the model's plan
        // does not contain, the recognized form line leads the plan and
        // the model's lines follow. "A giant stone statue of Zeus" from
        // a 1B was a tower and a tree; the words were all there, and
        // reading them is not something a 1B does. The model's reply is
        // still used -- nothing it wrote is dropped -- but the shape the
        // player asked for is guaranteed.
        // A recognized recipe leads the plan when the model's own reply
        // does not already do that kind of thing. For a form that means
        // "no form line"; for a CARVE it means no carve line -- and the
        // two are checked separately, or a model that answered a
        // "carve a schism" with a statue would have its statue accepted
        // as sufficient. Which is exactly what happened.
        let has_form = ir.lines().any(|l| l.trim_start().to_lowercase().starts_with("form "));
        let carve_verbs = ["canyon", "cave", "mesa", "waterfall", "overhang", "gorge", "feature"];
        let has_carve = ir.lines().any(|l| carve_verbs.iter()
            .any(|v| l.trim_start().to_lowercase().starts_with(&format!("{v} "))));
        // A SKETCH IS ALREADY THE SHAPE. Its IR is spheres and cell
        // stamps and has no `form` line, so the recognizer concluded
        // nothing had been built and added a dictionary statue ON TOP
        // of the studied body -- two figures in the same place, and the
        // bland one drawn last.
        let recognized = if self.suppress_recognizer { None } else {
            crate::recipe::recognise(prose)
                .filter(|r| if r.carve { !has_carve } else { !has_form })
        };
        let recognized_why = recognized.as_ref().map(|r| format!(
            "RECOGNIZED {} from '{}': {}", r.form, r.because.join(", "), r.line()));
        let ir_with_form = match &recognized {
            // A carve REPLACES the model's reply rather than leading
            // it: a statue the model wrote when asked for a canyon is
            // not a bonus, it is the wrong answer, and leaving it in
            // puts a gargoyle at the bottom of the rift.
            Some(r) if r.carve =>
                format!("# {}\n{}\n", r.form, r.line()),
            Some(r) => format!("# {}\n{}\n{}", r.name.clone().unwrap_or_else(|| r.form.clone()), r.line(), ir),
            None => ir.to_string(),
        };
        let ir: &str = &ir_with_form;
        // A /like spec waiting: prepend it unless the plan has its own.
        let expanded = match self.pending_spec.take() {
            Some(spec) if !ir.lines().any(|l| l.trim_start().starts_with("spec")) =>
                crate::plan::expand_defs(&format!("{spec}\n{ir}")),
            _ => crate::plan::expand_defs(ir),
        };
        let ir: &str = &expanded;
        self.creative_seed = self.creative_seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let (first_plan, first_issues) = parse_plan_at(ir, Some(f.anchor));
        // SALVAGE. One bad line used to reject the whole plan; a 1B that
        // gets eight lines right and one wrong got nothing built. Now
        // the lines that parse run, the ones that do not are kept for a
        // repair turn, and only a plan with NO usable line is rejected.
        let sal = crate::recovery::salvage(ir, &first_issues);
        self.last_salvage = if sal.bad.is_empty() { None } else { Some(sal.clone()) };
        let (mut plan, mut issues) = if sal.bad.is_empty() {
            (first_plan, first_issues)
        } else if sal.is_total_loss() {
            self.pending = None;
            let msg = sal.bad.iter().map(|(n, l, w)| format!("line {n} `{l}`: {w}"))
                .collect::<Vec<_>>().join("\n");
            return Reply::Say(format!("plan rejected, no usable lines:\n{msg}\n\
                                       (/repair asks the model to fix them)"));
        } else {
            parse_plan_at(&sal.good, Some(f.anchor))
        };
        let refusals = self.gate_world_ops(&mut plan, prose);
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
        // Then reverse, exactly as a dry run would.
        let (from_y, chunk_size) = (self.from_y, self.chunk_size);
        let archive_in = self.archive.clone();
        let archive_out = std::cell::RefCell::new(archive_in.clone());
        let dry_run = |tools: &mut VoxelTools<W>, plan: &BuildPlan|
            -> (PlanRun, Vec<(i32, i32, i32)>, Vec<critique::Finding>) {
            let first = tools.edits.len();
            // lend the session's archive to the run; keep what it learned
            tools.archive = archive_out.borrow().clone();
            let live: PlanRun = run_plan(tools, plan, from_y, chunk_size, false);
            *archive_out.borrow_mut() = std::mem::take(&mut tools.archive);
            let placed: Vec<(i32, i32, i32)> = tools.edits[first..].iter()
                .flat_map(|e| e.before.iter().map(|&(x, y, z, _)| (x, y, z))).collect();
            let mut findings = critique::floating(tools.world, &placed);
            if let Some(feet) = player_feet {
                if let Some(f) = critique::entombs_player(&placed, feet) { findings.push(f); }
            }
            // REACHABILITY. A build's doors must open onto somewhere a
            // body can stand, and the player must be able to walk to
            // it. Every feature in this crate implicitly promises this
            // and nothing checked it: a village on a canyon rim with no
            // path down is six correct buildings and one wrong place.
            if !placed.is_empty() && placed.len() < 400_000 {
                let lo = placed.iter().fold((i32::MAX, i32::MAX, i32::MAX),
                    |a, p| (a.0.min(p.0), a.1.min(p.1), a.2.min(p.2)));
                let hi = placed.iter().fold((i32::MIN, i32::MIN, i32::MIN),
                    |a, p| (a.0.max(p.0), a.1.max(p.1), a.2.max(p.2)));
                // from outside the build, at the player's feet if known
                let outside = player_feet.unwrap_or((lo.0 - 4, hi.1 + 1, lo.2 - 4));
                let body = crate::reach::Body { budget: 40_000, ..Default::default() };
                let (bad, total) = crate::reach::unreachable_doors(tools.world, lo, hi, outside, &body);
                if !bad.is_empty() {
                    findings.push(critique::Finding {
                        kind: "unreachable",
                        message: format!("{} of {total} opening(s) cannot be walked to", bad.len()),
                        at: bad[0],
                    });
                }
            }
            let ids: Vec<u64> = tools.edits[first..].iter().map(|e| e.id).collect();
            for id in ids.into_iter().rev() { tools.undo_edit(id); }
            (live, placed, findings)
        };
        let (mut dry, mut placed, mut findings) = dry_run(tools, &plan);

        // FINISHING FROM THE WORDS. "Creepy" from the player and `form
        // statue` from a 1B: the shape came from the model, the
        // treatment comes from the adjective, over the box the first
        // dry run just measured. Then a second dry run so the preview
        // shows the finished thing and the critique judges it.
        let mut finishing_why: Vec<String> = Vec::new();
        if !placed.is_empty() {
            let lo = placed.iter().fold((i32::MAX, i32::MAX, i32::MAX),
                |a, p| (a.0.min(p.0), a.1.min(p.1), a.2.min(p.2)));
            let hi = placed.iter().fold((i32::MIN, i32::MIN, i32::MIN),
                |a, p| (a.0.max(p.0), a.1.max(p.1), a.2.max(p.2)));
            let (mut fin_ir, mut why) = crate::finish::from_prose(prose, &plan.ops, Some((lo, hi)));
            // THE SILHOUETTE CRITIC. A box with a flat top is the
            // signature of a generated building; a real one has
            // something against the sky. Only for buildings of a size
            // -- a fence post is a box and should stay one -- and only
            // when the player did not ask for a box.
            if let Some(sil) = crate::finish::silhouette(&placed) {
                let big_enough = (hi.0 - lo.0) >= 5 && (hi.2 - lo.2) >= 5 && (hi.1 - lo.1) >= 3;
                let asked_for_box = prose.to_lowercase().contains("box") || prose.to_lowercase().contains("cube")
                    || prose.to_lowercase().contains("plain");
                if sil.is_boxy() && big_enough && !asked_for_box {
                    let (b_ir, b_why) = crate::finish::break_silhouette((lo, hi), self.creative_seed);
                    fin_ir.push_str(&b_ir);
                    why.push(b_why);
                }
            }
            if !fin_ir.is_empty() {
                let (fin_plan, _) = parse_plan_at(&fin_ir, Some(f.anchor));
                plan.ops.extend(fin_plan.ops);
                finishing_why = why;
                let again = dry_run(tools, &plan);
                dry = again.0; placed = again.1; findings = again.2;
            }
        }
        // SPEC TUNING. If the model said what the build should measure
        // like, a pool of placers and destroyers is evolved toward it
        // over the placed cells, and the winner's edits join the plan.
        // The record of prose -> spec -> outcome is the training set.
        let spec_line = plan.ops.iter().find_map(|o| match o { Op::Spec(s) => Some(s.clone()), _ => None });
        if let (Some(line), false) = (spec_line, placed.is_empty()) {
            if let Ok(spec) = crate::spec::Spec::parse(&line) {
                let set: std::collections::BTreeSet<(i32, i32, i32)> = placed.iter().copied().collect();
                let lo = placed.iter().fold((i32::MAX, i32::MAX, i32::MAX), |a, p| (a.0.min(p.0), a.1.min(p.1), a.2.min(p.2)));
                let hi = placed.iter().fold((i32::MIN, i32::MIN, i32::MIN), |a, p| (a.0.max(p.0), a.1.max(p.1), a.2.max(p.2)));
                let block = crate::plan::resolve_block("stone").unwrap_or(1);
                match crate::spec::tune(&set, (lo, hi), f.anchor.1 - 1, &spec, block, self.creative_seed, 16, 12) {
                    Some(t) => {
                        // the winner's edits as IR: fills and carves the
                        // plan already knows how to run and undo
                        let mut ir = String::new();
                        for ch in &t.changes {
                            ir.push_str(&format!("fill {},{},{} {},{},{} block={}\n",
                                ch.at.0, ch.at.1, ch.at.2, ch.at.0, ch.at.1, ch.at.2,
                                if ch.to == 0 { "air" } else { "stone" }));
                        }
                        let (tune_plan, _) = parse_plan_at(&ir, None);
                        plan.ops.extend(tune_plan.ops);
                        finishing_why.push(format!("{}; now {}", t.summary(), t.after.describe()));
                        self.training.push(crate::spec::training_line(prose, &spec, &t));
                        let again = dry_run(tools, &plan);
                        dry = again.0; placed = again.1; findings = again.2;
                    }
                    None => finishing_why.push(format!("spec {} -- already met, or nothing improved it",
                                                       spec.describe())),
                }
            }
        }
        let _ = &placed;
        self.archive = archive_out.into_inner();
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
        for r in &refusals {
            echo.push_str(&format!("\nREFUSED: {r}"));
        }
        if let Some(s) = &self.last_salvage {
            echo.push_str(&format!("\nSKIPPED {} unusable line(s) -- /repair to have the model fix them:",
                                   s.bad.len()));
            for (n, l, w) in &s.bad {
                echo.push_str(&format!("\n  line {n} `{l}`: {w}"));
            }
        }
        for w in &dry.withdrawn {
            echo.push_str(&format!("\nWITHDRAWN: {}", w.message()));
        }
        self.last_plan_error = issues.iter().find(|i| i.severity == Severity::Error)
            .map(|i| i.message.clone());
        if let Some(r) = &recognized_why { echo.push_str(&format!("\n{r}")); }
        for w in &finishing_why {
            echo.push_str(&format!("\nFINISHING ({w})"));
        }
        if let Some(a) = plan.anchor {
            echo.push_str(&format!(
                "\npending at {:?} -- a marker shows where; /commit from anywhere", a));
        }
        self.pending = Some(Pending {
            plan, feature_id: f.feature_id.clone(), predicted_blocks: dry.blocks_changed,
            ir: ir.to_string(), prose: prose.to_string(),
            placed: placed.clone(),
            program: String::new(),
            pass_log: Vec::new(),
            extra_cells: Vec::new(),
            findings: findings.iter().map(|f| f.message.clone()).collect(),
            bounds: {
                let mut lo = (i32::MAX, i32::MAX, i32::MAX);
                let mut hi = (i32::MIN, i32::MIN, i32::MIN);
                for b in &dry.batches { for (x, y, z, _) in &b.blocks {
                    lo = (lo.0.min(*x), lo.1.min(*y), lo.2.min(*z));
                    hi = (hi.0.max(*x), hi.1.max(*y), hi.2.max(*z));
                }}
                if lo.0 == i32::MAX { None } else { Some((lo, hi)) }
            },
        });
        Reply::Preview { echo, blocks: dry.blocks_changed, feature_id: f.feature_id }
    }

    /// The pending plan's numbers for a run's round: blocks, ops, box,
    /// and the placed cells (for the marker's blob).
    pub fn pending_detail(&self) -> Option<(usize, usize, Option<((i32, i32, i32), (i32, i32, i32))>, Vec<(i32, i32, i32)>)> {
        let p = self.pending.as_ref()?;
        Some((p.predicted_blocks, p.plan.ops.iter().filter(|o| !matches!(o, Op::Spec(_))).count(),
              p.bounds, p.placed.clone()))
    }

    /// How far the pending plan is from its own spec, if it has one.
    pub fn pending_spec_distance(&self) -> Option<f32> {
        let p = self.pending.as_ref()?;
        let line = p.plan.ops.iter().find_map(|o| match o { Op::Spec(s) => Some(s.clone()), _ => None })?;
        let spec = crate::spec::Spec::parse(&line).ok()?;
        let cells: std::collections::BTreeSet<(i32, i32, i32)> = p.placed.iter().copied().collect();
        let ground = p.plan.anchor.map(|a| a.1 - 1).unwrap_or(63);
        Some(crate::spec::distance(&spec, &crate::spec::measure(&cells, ground)))
    }

    /// Where the pending plan will land, for a marker in the world.
    /// None when nothing is pending. The anchor is the plan's own, fixed
    /// at parse time; the box is the dry run's, so a marker can show
    /// extent as well as centre.
    pub fn pending_waypoint(&self) -> Option<Waypoint> {
        let p = self.pending.as_ref()?;
        let anchor = p.plan.anchor?;
        Some(Waypoint { anchor, name: p.plan.name.clone(), blocks: p.predicted_blocks,
                        bounds: p.bounds })
    }

    /// Forget the cached context. Call whenever the world under the
    /// focus may have changed: commit and undo do it themselves; a host
    /// applying cells from the hive or deferred queue should too.
    pub fn invalidate_context(&self) { *self.context_cache.borrow_mut() = None; }

    /// Plants committed since the last call, for the engine to register.
    /// None when there are none, so a host can skip the message.
    /// Sites a lazy scatter handed over, for the engine to hold.
    pub fn take_sites(&mut self) -> Option<Vec<(String, crate::scatter::Site)>> {
        if self.held_sites.is_empty() { None } else { Some(std::mem::take(&mut self.held_sites)) }
    }

    pub fn take_plants(&mut self) -> Option<Vec<crate::flora::PlantRequest>> {
        if self.planted.is_empty() { None } else { Some(std::mem::take(&mut self.planted)) }
    }

    /// The follow-up turn asking the model to fix only what it got
    /// wrong, or None if the last turn was clean.
    pub fn repair_prompt(&self) -> Option<String> {
        self.last_salvage.as_ref().map(|s| s.repair_prompt())
    }

    /// Merge a repair reply with the salvaged good lines into a full
    /// plan, ready for `receive_plan_for`.
    pub fn merge_repair(&self, fixed: &str) -> Option<String> {
        self.last_salvage.as_ref().map(|s| s.merge(fixed))
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
        // No stale-focus guard here either: the ops are absolute, the
        // plan is a waypoint, and the player may commit from anywhere.
        // See `pending_waypoint`.
        let _ = gaze;
        let first = tools.edits.len();
        tools.archive = self.archive.clone();
        let run = run_plan(tools, &p.plan, self.from_y, self.chunk_size, false);
        self.archive = std::mem::take(&mut tools.archive);
        // and the sketch's rasterised shapes, in the same commit
        if !p.extra_cells.is_empty() { let _ = tools.apply_changes_large(&p.extra_cells); }
        self.invalidate_context();
        self.last_ir = p.ir.clone();
        self.last_prose = p.prose.clone();
        // Plants this commit planted, for the engine to register. Kept
        // on the session until the host takes them, because the tools
        // here are a staging world and know nothing about a lifecycle.
        if !run.plants.is_empty() { self.planted.extend(run.plants.iter().cloned()); }
        if !run.sites.is_empty() { self.held_sites.extend(run.sites.iter().cloned()); }
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
        // The plan's own anchor. This used to read the gaze at commit,
        // which was fine while the guard forced the gaze to stay; with
        // commits from anywhere it was the player's new spot, and the
        // reveal ordered itself from there -- and a rebase would have
        // replayed the IR against a point the player had merely walked to.
        self.last_anchor = p.plan.anchor.unwrap_or_else(|| self.focus(gaze).anchor);
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
        self.invalidate_context();
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
    fn commit_builds_at_the_planned_spot_from_anywhere() {
        // The plan is a WAYPOINT. The old guard refused a commit unless
        // the player was still looking at the spot, which meant staring
        // at one place for the twenty seconds a model takes -- and a
        // statue that was planned, previewed and never built because
        // the player had turned round.
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        s.receive_plan(&mut t, &gaze_at(20, 5, 15.0), "tower @0,0 r=3 h=6\n");
        let wp = s.pending_waypoint().expect("a waypoint while pending");
        assert_eq!((wp.anchor.0, wp.anchor.2), (20, 5), "the marker is at the plan's anchor");
        assert!(wp.bounds.is_some(), "with the dry run's box");
        // the player has turned round and walked off
        let r = s.commit(&mut t, &gaze_at(300, 300, 80.0));
        match r { Reply::Committed { blocks, .. } => assert!(blocks > 0), o => panic!("{o:?}") }
        assert_ne!(t.world.get(23, 66, 5), 0, "built where it was PLANNED, not where they look");
        assert_eq!(t.world.get(303, 66, 305), 0, "and nothing at the new gaze");
        assert!(s.pending_waypoint().is_none(), "the marker goes away once built");
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

    #[test]
    fn the_context_tells_the_model_where_it_is() {
        let mut w = Flat::new();
        let t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        s.climate = (0.1, 0.8);   // freezing and wet
        let ctx = s.context(&t, &gaze_at(0, 0, 12.0), Some("grass"));
        assert!(ctx.contains("SITE:"), "{ctx}");
        assert!(ctx.contains("freezing"), "climate reaches the prompt: {ctx}");
        assert!(ctx.contains("snowy"), "and becomes a palette suggestion: {ctx}");
    }

    #[test]
    fn a_game_that_never_sets_the_climate_behaves_as_before() {
        let mut w = Flat::new();
        let t = VoxelTools::new(&mut w);
        let s = ChatSession::new(32, 200);
        let ctx = s.context(&t, &gaze_at(0, 0, 12.0), None);
        assert!(ctx.contains("temperate"), "the default is the middle: {ctx}");
    }

    #[test]
    fn the_model_is_shown_the_terrain_not_asked_to_imagine_it() {
        struct Slope(std::collections::BTreeMap<(i32,i32,i32), u16>);
        impl VoxelWorld for Slope {
            fn get(&self, x: i32, y: i32, z: i32) -> u16 {
                if let Some(&b) = self.0.get(&(x,y,z)) { return b; }
                if y <= 64 + x / 2 { 1 } else { 0 }
            }
            fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool {
                self.0.insert((x,y,z), b); true
            }
        }
        let mut w = Slope(Default::default());
        let t = VoxelTools::new(&mut w);
        let s = ChatSession::new(32, 250);
        let ctx = s.context(&t, &gaze_at(0, 0, 12.0), None);
        assert!(ctx.contains("TERRAIN"), "a picture of the ground: {ctx}");
        assert!(ctx.contains('@'), "with the player marked");
        assert!(ctx.contains("rises"), "and the slope named in words");
    }

    #[test]
    fn an_uninvited_world_change_is_stripped_but_the_build_survives() {
        // The failure this gate exists to prevent: asked for a garden on
        // a glacier, warm the glacier. The garden should still be built.
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        let g = gaze_at(0, 0, 10.0);
        let ir = "world hotter r=40\nscatter @-6,-6 @6,6 block=flower clusters=4 per=5\n";
        match s.receive_plan_gated(&mut t, &g, ir, None, "a garden here") {
            Reply::Preview { echo, blocks, .. } => {
                assert!(echo.contains("REFUSED"), "{echo}");
                assert!(echo.contains("adapt"), "and says why: {echo}");
                assert!(blocks > 0, "the garden is still built");
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn an_explicitly_requested_world_change_passes_the_gate() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        let g = gaze_at(0, 0, 10.0);
        match s.receive_plan_gated(&mut t, &g, "world hotter r=40\n", None,
                                   "make it warmer here") {
            Reply::Preview { echo, .. } => assert!(!echo.contains("REFUSED"), "{echo}"),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn fortify_my_base_needs_no_model_when_looking_at_the_base() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        let g = gaze_at(0, 0, 10.0);
        s.receive_plan(&mut t, &g, "# base\nenclose @-4,-4 @4,4 h=4 block=stone_bricks\n");
        s.commit(&mut t, &g);
        let ir = s.macro_ir("fortify my base", &g).expect("a macro, not a model call");
        assert!(ir.contains("# fortify base"));
        // and it builds
        match s.receive_plan(&mut t, &g, &ir) {
            Reply::Preview { blocks, .. } => assert!(blocks > 100, "walls and towers: {blocks}"),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn a_macro_with_nothing_to_apply_to_falls_through_to_the_model() {
        let mut w = Flat::new();
        let t = VoxelTools::new(&mut w);
        let s = ChatSession::new(32, 200);
        assert!(s.macro_ir("fortify my base", &gaze_at(0, 0, 10.0)).is_none(),
                "no base yet: the model should build one");
        let _ = t;
    }

    #[test]
    fn a_plan_with_one_bad_line_still_previews_and_offers_repair() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        let g = gaze_at(0, 0, 10.0);
        let ir = "enclose @-4,-4 @4,4 h=4 block=oak_planks\nfrobnicate the widget\ntree @8,8 oak\n";
        match s.receive_plan(&mut t, &g, ir) {
            Reply::Preview { echo, blocks, .. } => {
                assert!(blocks > 0, "the good lines build");
                assert!(echo.contains("SKIPPED 1"), "{echo}");
                assert!(echo.contains("frobnicate"), "{echo}");
            }
            o => panic!("{o:?}"),
        }
        let rp = s.repair_prompt().expect("a repair turn is available");
        assert!(rp.contains("frobnicate") && !rp.contains("enclose"));
        let merged = s.merge_repair("tree @2,2 birch h=6").unwrap();
        assert!(merged.contains("enclose") && merged.contains("birch"));
    }

    #[test]
    fn a_structural_failure_withdraws_its_dependents_in_the_preview() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        let g = gaze_at(0, 0, 10.0);
        // a cylinder too large to place, then a roof that would stand on it
        let ir = "cylinder @0,0 r=90 h=200 block=stone\nroof @-4,-4 @4,4 gable\n";
        match s.receive_plan(&mut t, &g, ir) {
            Reply::Preview { echo, .. } =>
                assert!(echo.contains("WITHDRAWN"), "the roof is withdrawn: {echo}"),
            Reply::Say(m) => assert!(m.contains("rejected"), "{m}"),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn refining_the_last_build_needs_no_model_and_actually_changes_it() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        let g = gaze_at(0, 0, 10.0);
        s.receive_plan(&mut t, &g, "# hut\nenclose @-5,-5 @5,5 h=5 block=stone\n");
        s.commit(&mut t, &g);
        let before = (60..75).flat_map(|y| (-6..=6).map(move |x| (x, y)))
            .filter(|(x, y)| t.world.get(*x, *y, -5) != 0).count();

        let rungs = s.refine_ladder(&t, &g).expect("a build to refine");
        assert_eq!(rungs.len(), 5);
        // run the first two rungs and check the wall actually changed
        for r in rungs.iter().take(2) {
            match s.receive_plan(&mut t, &g, &r.ir) {
                Reply::Preview { blocks, .. } =>
                    assert!(blocks > 0, "rung '{}' changed nothing", r.name),
                o => panic!("rung '{}': {o:?}", r.name),
            }
            s.commit(&mut t, &g);
        }
        let mixed: std::collections::BTreeSet<u16> = (60..75)
            .flat_map(|y| (-6..=6).map(move |x| (x, y)))
            .map(|(x, y)| t.world.get(x, y, -5))
            .filter(|b| *b != 0).collect();
        assert!(mixed.len() > 1, "the wall is no longer one material: {mixed:?}");
        assert!(before > 0);
    }

    #[test]
    fn connect_resolves_two_builds_by_name_in_the_order_spoken() {
        let mut land = crate::land::Registry::new();
        land.claim("Old Mill", "kess", (0, 64, 0), (8, 70, 8));
        land.claim("Red Barn", "kess", (60, 64, 20), (70, 72, 30));
        land.claim("Well", "kess", (30, 64, 30), (32, 66, 32));
        let ir = ChatSession::connect_ir("connect the mill to the barn", &land).unwrap();
        assert!(ir.contains("road 4,4 65,25"), "mill -> barn: {ir}");
        let ir2 = ChatSession::connect_ir("a road from the barn to the mill", &land).unwrap();
        assert!(ir2.contains("road 65,25 4,4"), "barn -> mill: {ir2}");
        assert!(ChatSession::connect_ir("connect the mill to the tower", &land).is_err());
        let e = ChatSession::connect_ir("connect x to y", &land).unwrap_err();
        assert!(e.contains("Old Mill"), "lists what exists: {e}");
    }

    #[test]
    fn committed_plants_reach_the_host_once() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        let g = gaze_at(0, 0, 10.0);
        s.receive_plan(&mut t, &g, "plant @0,0 oak tall\nplant @8,8 birch\n");
        assert!(s.take_plants().is_none(), "a preview registers nothing");
        s.commit(&mut t, &g);
        let p = s.take_plants().expect("the commit's plants");
        assert_eq!(p.len(), 2);
        assert!(s.take_plants().is_none(), "taken once");
    }

    #[test]
    fn a_bad_line_after_a_multi_line_use_is_still_salvaged_correctly() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        let g = gaze_at(0, 0, 10.0);
        let ir = "def hut\nenclose @-3,-3 @3,3 h=4\nroof @-3,-3 @3,3\nend\nuse hut @0,0\nfrobnicate\n";
        match s.receive_plan(&mut t, &g, ir) {
            Reply::Preview { echo, blocks, .. } => {
                assert!(blocks > 0, "the hut builds");
                assert!(echo.contains("SKIPPED 1") && echo.contains("frobnicate"), "{echo}");
                assert!(!echo.contains("`roof") && !echo.contains("`enclose"),
                        "the right line was dropped, not a hut line: {echo}");
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn the_context_is_cached_by_focus_and_invalidated_by_a_change() {
        let mut w = Flat::new();
        let mut s = ChatSession::new(32, 200);
        let g = gaze_at(0, 0, 10.0);
        let a = { let t = VoxelTools::new(&mut w); s.context(&t, &g, Some("grass")) };
        let b = { let t = VoxelTools::new(&mut w); s.context(&t, &g, Some("grass")) };
        assert_eq!(a, b);
        assert!(s.context_cache.borrow().is_some(), "served from the cache the second time");
        // the world changes under the focus. The feature id does NOT
        // change (it is gaze and tier, not contents), so the host must
        // invalidate -- and after it does, the new terrain is read.
        for x in -3..=3 { for z in -3..=3 { w.set(x, 66, z, 1); } }
        let stale = { let t = VoxelTools::new(&mut w); s.context(&t, &g, Some("grass")) };
        assert_eq!(stale, a, "without invalidation the cache is served (and would be stale)");
        s.invalidate_context();
        let c = { let t = VoxelTools::new(&mut w); s.context(&t, &g, Some("grass")) };
        assert_ne!(a, c, "after invalidation the build under the focus is read");
        // and the climate changing invalidates it too
        s.climate = (0.1, 0.9);
        let d = { let t = VoxelTools::new(&mut w); s.context(&t, &g, Some("grass")) };
        assert_ne!(c, d);
        assert!(d.contains("freezing"));
    }

    #[test]
    fn creepy_finishes_a_plain_statue_from_the_adjective() {
        // The 1B said `form statue`; the player said "creepy". Neither
        // alone gives a creepy statue.
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        let g = gaze_at(0, 0, 10.0);
        let plain = match s.receive_plan_gated(&mut t, &g, "form statue @0,0 @6,6 gargoyle\n", None,
                                               "a stone statue") {
            Reply::Preview { echo, blocks, .. } => { assert!(!echo.contains("FINISHING"), "{echo}"); blocks }
            o => panic!("{o:?}"),
        };
        match s.receive_plan_gated(&mut t, &g, "form statue @0,0 @6,6 gargoyle\n", None,
                                   "a creepy stone statue") {
            Reply::Preview { echo, blocks, .. } => {
                assert!(echo.contains("FINISHING (creepy"), "{echo}");
                assert!(echo.contains("mood eerie") || echo.contains("mood 'eerie'") || echo.contains("eerie"), "{echo}");
                assert_ne!(blocks, plain, "the finishing changed the result");
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn a_commit_remembers_its_ir_and_prose_for_the_journal() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        let g = gaze_at(0, 0, 10.0);
        s.receive_plan_gated(&mut t, &g, "cylinder @0,0 r=2 h=5\n", None, "a little tower");
        assert!(s.last_ir.is_empty(), "nothing committed yet");
        s.commit(&mut t, &g);
        assert!(s.last_ir.contains("cylinder"));
        assert_eq!(s.last_prose, "a little tower");
    }

    #[test]
    fn the_committed_anchor_is_the_plans_not_the_gaze_at_commit() {
        // A tower of r=3 at anchor (20,·,5) has its box minimum at
        // (17,·,2). Replaying `cylinder @0,0` against (17,2) would put
        // the rebased tower three blocks off -- and replaying against
        // where the player STOOD at commit would put it anywhere.
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        let g = gaze_at(20, 5, 15.0);
        s.receive_plan(&mut t, &g, "cylinder @0,0 r=3 h=6\n");
        // committed from somewhere else entirely
        s.commit(&mut t, &gaze_at(300, 300, 80.0));
        let a = s.last_anchor;
        assert_eq!((a.0, a.2), (20, 5));
        let lb = s.last_build.as_ref().unwrap();
        assert_ne!((lb.min.0, lb.min.2), (20, 5), "and it is not the box corner");
    }

    #[test]
    fn a_plain_box_gets_its_silhouette_broken_unless_a_box_was_asked_for() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        let g = gaze_at(0, 0, 10.0);
        let ir = "fill @-5,0,-4 @5,6,4 block=stone\n";
        match s.receive_plan_gated(&mut t, &g, ir, None, "a hall") {
            Reply::Preview { echo, .. } => assert!(echo.contains("boxy silhouette"), "{echo}"),
            o => panic!("{o:?}"),
        }
        match s.receive_plan_gated(&mut t, &g, ir, None, "a plain stone box") {
            Reply::Preview { echo, .. } => assert!(!echo.contains("boxy"), "asked for a box: {echo}"),
            o => panic!("{o:?}"),
        }
        // a fence post is a box and stays one
        match s.receive_plan_gated(&mut t, &g, "fill @0,0,0 @0,3,0 block=oak_log\n", None, "a post") {
            Reply::Preview { echo, .. } => assert!(!echo.contains("boxy"), "{echo}"),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn the_same_request_twice_is_not_the_same_building() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        let g = gaze_at(0, 0, 10.0);
        let ir = "fill @-5,0,-4 @5,6,4 block=stone\n";
        let a = match s.receive_plan_gated(&mut t, &g, ir, None, "a hall") { Reply::Preview { echo, .. } => echo, o => panic!("{o:?}") };
        let b = match s.receive_plan_gated(&mut t, &g, ir, None, "a hall") { Reply::Preview { echo, .. } => echo, o => panic!("{o:?}") };
        let c = match s.receive_plan_gated(&mut t, &g, ir, None, "a hall") { Reply::Preview { echo, .. } => echo, o => panic!("{o:?}") };
        let pick = |e: &str| e.lines().find(|l| l.contains("boxy")).unwrap_or("").to_string();
        assert!(pick(&a) != pick(&b) || pick(&b) != pick(&c), "the breaker varies across turns:\n{}\n{}\n{}", pick(&a), pick(&b), pick(&c));
    }

    #[test]
    fn a_spec_tunes_the_build_and_writes_a_training_line() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 200);
        let g = gaze_at(0, 0, 10.0);
        // an open box asked to be sealed and durable
        let ir = "spec durability=high sound=muffled shelter=high\nenclose @-4,-4 @4,4 h=5 block=stone\n";
        match s.receive_plan_gated(&mut t, &g, ir, None, "a bunker") {
            Reply::Preview { echo, .. } => {
                assert!(echo.contains("spec-tuned"), "{echo}");
                assert!(echo.contains("target:"), "the echo shows the target: {echo}");
            }
            o => panic!("{o:?}"),
        }
        assert_eq!(s.training.len(), 1);
        assert!(s.training[0].starts_with("a bunker\t"));
    }

    #[test]
    fn like_the_mill_but_taller_becomes_a_spec() {
        use crate::vcs::Repo;
        use crate::weather::Change;
        let mut repo = Repo::new(32);
        let hut: Vec<Change> = (0..9).flat_map(|x| (65..70).flat_map(move |y| (0..9).filter_map(move |z| {
            let wall = x == 0 || x == 8 || z == 0 || z == 8 || y == 69;
            wall.then(|| Change { at: (x, y, z), to: 57 }) }))).collect();
        repo.commit_changes("k", "the old mill", "", &hut);
        let mut idx = crate::latent::Index::new();
        idx.index_branch(&repo, "main", 64);
        let (line, note) = ChatSession::like_spec("something like the mill but taller and sturdier", &idx).unwrap();
        assert!(line.starts_with("spec "), "{line}");
        assert!(note.contains("mill") && note.contains("taller"), "{note}");
        let spec = crate::spec::Spec::parse(&line).unwrap();
        assert!(spec.height.unwrap().0 > 5, "taller than the 5-high mill: {:?}", spec.height);
        assert!(ChatSession::like_spec("a plain tower", &idx).is_none());
        assert!(ChatSession::like_spec("like the mill", &idx).is_some(), "at the start of a command");
        assert!(ChatSession::like_spec("like the cathedral", &idx).is_none(), "no such build");
    }

    #[test]
    fn a_named_build_is_in_the_context_beside_the_last_one() {
        let mut w = Flat::new();
        let mut s = ChatSession::new(32, 200);
        s.last_build = Some(LastBuild { name: "the well".into(), min: (0, 65, 0), max: (2, 67, 2), blocks: 20, palette_hint: None });
        s.referents.push(LastBuild { name: "the mill".into(), min: (40, 65, 40), max: (52, 74, 50), blocks: 900, palette_hint: None });
        let ctx = { let t = VoxelTools::new(&mut w); s.context(&t, &gaze_at(0, 0, 10.0), None) };
        assert!(ctx.contains("LAST BUILD 'the well'"), "{ctx}");
        assert!(ctx.contains("NAMED BUILD 'the mill': box (40, 65, 40)..(52, 74, 50)"), "{ctx}");
    }

    #[test]
    fn the_zeus_sentence_gets_a_statue_even_from_a_tower_and_a_tree() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 400_000);
        let g = gaze_at(0, 0, 10.0);
        // exactly what the 1B answered
        let ir = "cylinder @2,-1 r=2 h=10 block=stone\ntree @6,4 oak h=8\n";
        match s.receive_plan_gated(&mut t, &g, ir, None,
                                   "a giant stone statue of Zeus holding a lightning bolt in a lunge combat position") {
            Reply::Preview { echo, blocks, .. } => {
                assert!(echo.contains("RECOGNIZED statue"), "{echo}");
                assert!(echo.contains("pose=lunge") && echo.contains("hold=bolt") && echo.contains("beard=1"), "{echo}");
                assert!(blocks > 3000, "a colossus, not a pillar: {blocks}");
                assert!(echo.contains("tower") || echo.contains("cylinder"), "the model's own lines still ran: {echo}");
            }
            o => panic!("{o:?}"),
        }
        // a plan that already has a form is left alone
        match s.receive_plan_gated(&mut t, &g, "form statue @-3,-3 @3,3 idol\n", None, "a giant statue") {
            Reply::Preview { echo, .. } => assert!(!echo.contains("RECOGNIZED"), "{echo}"),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn the_archive_grows_across_turns_and_seeds_the_next() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 400_000);
        let g = gaze_at(0, 0, 10.0);
        assert!(s.archive.is_empty());
        s.receive_plan_gated(&mut t, &g, "enclose @-5,-5 @5,5 h=6 block=stone\ngreeble @-6,-1,-6 @6,7,6 gothic gens=6 levels=2\n",
                             None, "a gothic hall");
        assert!(!s.archive.is_empty(), "the preview's evolution left elites: {}", s.archive.len());
        assert!(!s.archive.seeds("greeble/gothic/L0").is_empty());
        let n = s.archive.len();
        s.receive_plan_gated(&mut t, &g, "enclose @20,-5 @30,5 h=6 block=stone\ngreeble @19,-1,-6 @31,7,6 gothic gens=6 levels=2\n",
                             None, "another gothic hall");
        assert!(s.archive.len() >= n.min(6), "kept or grew, capped: {}", s.archive.len());
    }

    #[test]
    fn a_review_sees_the_build_and_a_verdict_replans_it() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 400_000);
        let g = gaze_at(0, 0, 10.0);
        s.receive_plan_gated(&mut t, &g, "enclose @-4,-4 @4,4 h=5 block=stone\ntree @9,9 oak h=7\n", None, "a hall");
        let p = s.review_prompt(1, &[]).expect("a prompt for the pending plan");
        assert!(p.contains("REVIEW round 1") && p.contains("The player asked for: a hall"), "{p}");
        assert!(p.contains("1. ") && p.contains("2. "));
        assert!(p.contains("FRONT (") && p.contains("SIDE ("));
        assert!(p.contains("Measured:"));
        assert!(!p.contains("@-4,-4"), "no coordinates in the review: {p}");
        // the model drops the tree and adds a roof
        let before = s.pending_waypoint().unwrap().blocks;
        let (r, changed, review) = s.apply_review(&mut t, &g, "drop 2\nadd roof @-4,-4 @4,4 gable\n", None);
        assert!(changed, "{review:?}");
        match r { Reply::Preview { echo, blocks, .. } => {
            assert!(!echo.contains("tree"), "the tree is gone: {echo}");
            assert!(echo.contains("roof") || echo.contains("gable"), "the roof arrived: {echo}");
            assert_ne!(blocks, before);
        } o => panic!("{o:?}") }
        // "done" changes nothing
        let (_, changed, review) = s.apply_review(&mut t, &g, "done\n", None);
        assert!(!changed && review.done);
    }

    #[test]
    fn asked_to_carve_the_model_does_not_get_to_build_a_statue_instead() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 400_000);
        let g = gaze_at(0, 0, 10.0);
        // exactly the 1B's reply from the screenshot
        let ir = "form statue gargoyle over 5x5 height=18\n";
        match s.receive_plan_gated(&mut t, &g, ir, None, "carve a great schism into the ground") {
            Reply::Preview { echo, blocks, .. } => {
                assert!(echo.contains("RECOGNIZED canyon"), "{echo}");
                assert!(!echo.to_lowercase().contains("statue"), "the statue is gone: {echo}");
                assert!(blocks > 5_000, "a schism, not 50 blocks: {blocks}");
            }
            o => panic!("{o:?}"),
        }
        // and a model that DOES carve is left alone
        match s.receive_plan_gated(&mut t, &g, "canyon @-40,0 @40,0 w=12 depth=18\n", None, "carve a rift") {
            Reply::Preview { echo, .. } => assert!(!echo.contains("RECOGNIZED"), "{echo}"),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn a_water_sphere_becomes_a_water_sphere_and_not_a_house() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 400_000);
        let g = gaze_at(0, 0, 10.0);
        // pass 2's output, as a model would write it
        let sketch = "part shell sphere r=8 material=glass hollow\n\
                      part water sphere r=7 material=water\n\
                      part core sphere r=2 material=ice\n\
                      rel shell above ground by=10\n\
                      rel water inside shell\n\
                      rel core inside water\n";
        match s.receive_sketch(&mut t, &g, sketch, None, "a water sphere") {
            Reply::Preview { echo, blocks, .. } => {
                assert!(blocks > 1000, "a sphere of some size: {blocks}");
                let e = echo.to_lowercase();
                assert!(!e.contains("house") && !e.contains("cottage"), "no house: {echo}");
                // the sphere op calls itself a dome in its arithmetic echo
                assert!(e.contains("sphere") || e.contains("dome"), "{echo}");
            }
            o => panic!("{o:?}"),
        }
        s.commit(&mut t, &g);
        drop(t);
        // it is where it was asked to be: floating, and made of water
        let mut water = 0; let mut glass = 0;
        for y in 60..100 { for x in -12..12 { for z in -12..12 {
            match w.get(x, y, z) { 9 => water += 1, 61 => glass += 1, _ => {} }
        }}}
        assert!(water > 500, "{water} water blocks");
        assert!(glass > 200, "{glass} glass blocks in the shell");
    }

    #[test]
    fn a_sketch_with_a_shape_the_ir_cannot_say_still_commits_as_one_edit() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 400_000);
        let g = gaze_at(0, 0, 10.0);
        // a torus has no IR op: it rides as cells
        let sketch = "part shaft cylinder r=2 h=20 material=stone\n\
                      part ring torus r=9 thickness=1 material=quartz\n\
                      rel shaft on ground\n\
                      rel ring around shaft\n";
        let before = match s.receive_sketch(&mut t, &g, sketch, None, "a ring around a shaft") {
            Reply::Preview { blocks, .. } => blocks,
            o => panic!("{o:?}"),
        };
        assert!(before > 300, "the ring is counted in the preview: {before}");
        s.commit(&mut t, &g);
        let mut quartz = 0;
        {
        let ww = &*t.world;
        for y in 60..95 { for x in -14..14 { for z in -14..14 {
            if ww.get(x, y, z) == 51 { quartz += 1; }
        }}}
        }
        assert!(quartz > 100, "the ring is really there: {quartz}");
        // and undo takes the whole thing, ring included
        s.undo(&mut t);
        drop(t);
        let mut left = 0;
        for y in 65..95 { for x in -14..14 { for z in -14..14 {
            if w.get(x, y, z) != 0 { left += 1; }
        }}}
        assert_eq!(left, 0, "undo removed the ring too: {left} left");
    }

    #[test]
    fn a_program_carves_where_it_says_cut_and_builds_where_it_says_add() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 400_000);
        let g = gaze_at(0, 0, 10.0);
        // the schism, as the new pass would plan it: ONE CUT PASS
        let prog = "pass rift cut\n\
                      part gash box w=80 h=24 d=14 material=stone\n\
                      rel gash below ground by=0\n\
                    end\n";
        match s.receive_program(&mut t, &g, prog, None, "carve a great schism into the ground") {
            Reply::Preview { echo, blocks, .. } => {
                assert!(blocks > 3000, "a schism: {blocks}");
                assert!(echo.contains("rift (cut"), "{echo}");
                assert!(echo.contains("0 added"), "a pure cut adds nothing: {echo}");
            }
            o => panic!("{o:?}"),
        }
        s.commit(&mut t, &g);
        drop(t);
        // the ground below the anchor is gone
        assert_eq!(w.get(0, 60, 0), 0, "cut through");
        assert_ne!(w.get(60, 60, 0), 0, "and only where it said");
    }

    #[test]
    fn a_program_alternates_passes_over_one_region() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 400_000);
        let g = gaze_at(0, 0, 10.0);
        // mass, then cut the gate, then add the merlons: three sweeps
        let prog = "asset merlon\n\
                      part tooth box w=2 h=3 d=2 material=stone\n\
                    end\n\
                    pass mass add\n\
                      part keep box w=20 h=14 d=20 material=stone hollow\n\
                      rel keep on ground\n\
                    end\n\
                    pass gate cut\n\
                      part door box w=4 h=6 d=4 material=stone\n\
                      rel door on ground\n\
                    end\n\
                    pass teeth add\n\
                      repeat merlon line from=-9,7,-10 to=9,7,-10 step=4\n\
                    end\n";
        match s.receive_program(&mut t, &g, prog, None, "a keep with a gate and merlons") {
            Reply::Preview { echo, blocks, .. } => {
                assert!(blocks > 500, "{blocks}");
                // all three passes ran, in order, with their polarities
                assert!(echo.contains("mass (add") && echo.contains("gate (cut")
                        && echo.contains("teeth (add"), "{echo}");
                assert!(echo.contains("3 pass(es)"), "{echo}");
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn a_program_can_be_reviewed_and_its_polarity_fixed() {
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 400_000);
        let g = gaze_at(0, 0, 10.0);
        // the commonest fault: a gate pass that ADDS where it should cut
        let prog = "pass mass add\n\
                      part keep box w=20 h=14 d=20 material=stone hollow\n\
                      rel keep on ground\n\
                    end\n\
                    pass gate add\n\
                      part door box w=4 h=7 d=22 material=stone\n\
                      rel door on ground\n\
                    end\n";
        s.receive_program(&mut t, &g, prog, None, "a keep with a gate through it");
        // the review sees PASSES, not an empty op list
        let p = s.review_prompt(1, &[]).expect("a prompt");
        assert!(p.contains("1. mass (add"), "{p}");
        assert!(p.contains("2. gate (add"), "{p}");
        assert!(p.contains("The program:"), "the source, so a pass can be rewritten");
        assert!(!p.contains("The plan, by number"), "not the op-based prompt");
        // the model spots the polarity and rewrites the pass
        let (r, changed, review) = s.apply_review(&mut t, &g,
            "replace 2 gate cut\n  part door box w=4 h=7 d=22 material=stone\n  rel door on ground\nend\n",
            None);
        assert!(changed, "{review:?}");
        match r {
            Reply::Preview { echo, .. } => {
                assert!(echo.contains("gate (cut"), "the gate cuts now: {echo}");
                assert!(echo.contains("mass (add"), "and the mass is untouched");
            }
            o => panic!("{o:?}"),
        }
        s.commit(&mut t, &g);
        drop(t);
        // there is a hole through the keep where the gate is
        // the gate cut a hole through the south wall; the shell of a
        // hollow box is at +-10, so x=9 is interior either way
        assert_eq!(w.get(0, 67, -10), 0, "the gate is open through the wall");
        assert_ne!(w.get(10, 67, -10), 0, "and the wall stands beside it");
    }

    #[test]
    fn a_sketch_does_not_also_get_a_dictionary_statue_on_top_of_it() {
        // a sketch's IR is spheres and cell stamps with no `form` line,
        // so the recognizer concluded nothing had been built and added
        // one: two figures in the same place, the bland one last
        let mut w = Flat::new();
        let mut t = VoxelTools::new(&mut w);
        let mut s = ChatSession::new(32, 400_000);
        let g = gaze_at(0, 0, 10.0);
        let sketch = "part torso capsule r=4 r2=3 len=18 pitch=90 material=stone\n\
                      part head sphere r=3 material=stone\n\
                      rel torso above ground by=16\n\
                      rel head on torso\n";
        match s.receive_sketch(&mut t, &g, sketch, None, "a colossal statue of Zeus") {
            Reply::Preview { echo, .. } => {
                assert!(!echo.contains("RECOGNIZED"), "the sketch is the shape: {echo}");
                assert!(!echo.contains("form 'statue'"), "{echo}");
            }
            o => panic!("{o:?}"),
        }
        // and the flag does not stick: a plain build still recognises
        assert!(!s.suppress_recognizer, "the suppression is scoped to the sketch");
        match s.receive_plan_gated(&mut t, &g, "sphere 0,0 r=2 block=stone\n", None, "a stone house") {
            Reply::Preview { echo, .. } => assert!(echo.contains("RECOGNIZED"), "{echo}"),
            o => panic!("{o:?}"),
        }
    }
}
