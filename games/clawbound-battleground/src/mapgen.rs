//! mapgen -- the surface an AI authors a map through.
//!
//! Modelled on unendlich's `voxel-tools`, whose rules were learned the hard
//! way and are adopted here rather than rediscovered:
//!
//! * **The model cannot see the world.** Reads are DIGESTS ([`MapDraft::survey`]):
//!   a few lines of counts, fractions and distances, never coordinates dumps.
//! * **Writes are bounded and refused in words.** A bad call does not
//!   half-apply and does not panic; it returns a sentence the model can act
//!   on ("inside the red mid outer tower's cleared ground").
//! * **Every call is undoable**, as a group.
//! * **The loop is visible.** [`run`] produces a transcript of call, result,
//!   call, result -- readable, and steerable by pasting a line in.
//!
//! One rule is this game's own: a placement is MIRRORED through the origin
//! by construction (the symmetry the two bases have), so a map cannot be
//! authored unfair, even by accident.
//!
//! The author is a trait of `&str -> Option<String>`: a script in tests, a
//! person at a prompt, or a model behind `ai-gpu` -- with no dependency from
//! here on any of them. The protocol is one call per line, because a line is
//! what every one of those can produce:
//!
//!   place <Kind> <x> <z> [quarter]     survey <x> <z> <radius>
//!   undo                                validate          done

use crate::blockers::{Blockers, PropKind};
use crate::consts;
use crate::midjungle::Placement;

pub const MAX_PLACEMENTS: usize = 400;
pub const MAX_STEPS: usize = 300;

/// What may be placed: exactly what `scene::build_world` has a mesh arm for.
pub const KINDS: [(&str, PropKind); 10] = [
    ("Rock", PropKind::Rock), ("BoulderCluster", PropKind::BoulderCluster), ("RuinPillar", PropKind::RuinPillar),
    ("StoneHead", PropKind::StoneHead), ("FallenLog", PropKind::FallenLog), ("JungleTree", PropKind::JungleTree),
    ("StranglerFig", PropKind::StranglerFig), ("Palm", PropKind::Palm), ("Fern", PropKind::Fern), ("Bush", PropKind::Bush),
];

/// Where a placement's twin goes: the active map's own symmetry.
pub fn mirror(x: f32, z: f32) -> (f32, f32) {
    match crate::map::active() {
        crate::map::MapId::Jungle => (-x, -z),
        crate::map::MapId::Pass => (x, -z),
    }
}

/// The tool descriptions, for a system prompt.
pub fn tools() -> String {
    format!(
        "You author the scenery of {} by calling tools, one per line. You cannot see the map; ask.\n\
         place <Kind> <x> <z> [quarter 0-3]  -- place a prop AND its mirror ({}). Kinds: {}.\n\
         survey <x> <z> <radius>             -- a digest of what is there.\n\
         undo                                -- take back the last placement (both halves).\n\
         validate                            -- check the whole map.\n\
         done                                -- finish.\n\
         Units are world units; the arena is {:.0} across; +z is the red base. Refusals say why.",
        crate::map::active().name(),
        match crate::map::active() {
            crate::map::MapId::Jungle => "through the origin",
            crate::map::MapId::Pass => "across z = 0; the ground is a canyon floor and everything else is cliff",
        },
        KINDS.iter().map(|k| k.0).collect::<Vec<_>>().join(", "),
        consts::ARENA_RADIUS * 2.0
    )
}

#[derive(Debug, Clone, Default)]
pub struct MapDraft {
    pub placements: Vec<Placement>,
    /// How many placements each successful `place` added, for undo.
    groups: Vec<usize>,
}

fn solid(k: PropKind) -> bool {
    k.footprint().radius > 0.0 && !matches!(k, PropKind::Fern | PropKind::Bush)
}

impl MapDraft {
    /// Why `(kind, x, z)` may not stand there, or `None` if it may.
    pub fn refusal(&self, kind: PropKind, x: f32, z: f32) -> Option<String> {
        if !x.is_finite() || !z.is_finite() {
            return Some("coordinates must be numbers".into());
        }
        let r = kind.footprint().radius;
        if x.hypot(z) > consts::arena_radius() - 4.0 {
            return Some(format!("outside the arena (radius {:.0})", consts::arena_radius()));
        }
        // On a map carved out of rock, the rock is not floor. Without this
        // the engine accepted a rock on top of Twinfang Pass's 96 u summit
        // and validate() passed it (examples/mapgen_eval).
        if let Some(f) = crate::map::floor() {
            let d = f.sdf(x, z);
            if d > -(r.max(0.5)) {
                return Some(if d > 0.0 {
                    format!("in the rock: that is cliff {d:.1} u from the nearest floor, not ground to stand on")
                } else {
                    format!("on the very edge of the floor ({:.1} u in); a {kind:?} needs {:.1}", -d, r.max(0.5))
                });
            }
        }
        let lane = consts::lane_clearance(x, z);
        let need = if solid(kind) { r + 3.0 } else { 2.0 };
        if lane < need {
            return Some(format!("{lane:.1} from a lane's centreline; a {kind:?} needs {need:.1} so minions can pass"));
        }
        if consts::in_tower_clearing(x, z) {
            return Some(format!("inside a tower's cleared ground ({:.0} u): nothing that hides or lifts a cat stands there", consts::TOWER_CLEARING));
        }
        if consts::river_dist(x, z) < 6.0 {
            return Some("in or on the bank of a river".into());
        }
        if crate::bosses::in_pit(x, z) {
            return Some("inside a boss pit".into());
        }
        if crate::logs::crossings().iter().any(|c| (c.at.x - x).hypot(c.at.z - z) < 14.0) {
            return Some("too near a river crossing; the log needs room to roll".into());
        }
        if solid(kind) {
            if let Some(p) = self.placements.iter().find(|p| solid(p.kind) && (p.x - x).hypot(p.z - z) < p.kind.footprint().radius + r + 0.3) {
                return Some(format!("overlaps the {:?} at ({:.1}, {:.1})", p.kind, p.x, p.z));
            }
        }
        None
    }

    /// Place a prop and its twin. All or nothing.
    pub fn place(&mut self, kind: PropKind, x: f32, z: f32, quarter: u8) -> Result<String, String> {
        if self.placements.len() + 2 > MAX_PLACEMENTS {
            return Err(format!("the map is full ({MAX_PLACEMENTS} props)"));
        }
        if let Some(why) = self.refusal(kind, x, z) {
            return Err(why);
        }
        // The twin is the map's own mirror image: through the origin on the
        // jungle, across z = 0 on the pass (its lanes and clearings are
        // laid out that way; a point mirror of a pass clearing can land on
        // the cliff edge).
        let (tx, tz) = mirror(x, z);
        let own_twin = (x - tx).hypot(z - tz) < kind.footprint().radius.max(1.0);
        let before = self.placements.len();
        self.placements.push(Placement { kind, x, z, quarter: quarter % 4, scale: 1.0 });
        if !own_twin {
            if let Some(why) = self.refusal(kind, tx, tz) {
                self.placements.truncate(before);
                return Err(format!("its mirror at ({tx:.1}, {tz:.1}) is refused: {why}"));
            }
            self.placements.push(Placement { kind, x: tx, z: tz, quarter: (quarter + 2) % 4, scale: 1.0 });
        }
        self.groups.push(self.placements.len() - before);
        Ok(format!("placed {kind:?} at ({x:.1}, {z:.1}){}", if own_twin { "" } else { " and its mirror" }))
    }

    pub fn undo(&mut self) -> Result<String, String> {
        let n = self.groups.pop().ok_or("nothing to undo")?;
        let gone = self.placements.split_off(self.placements.len() - n);
        Ok(format!("took back the {:?}", gone[0].kind))
    }

    pub fn blockers(&self) -> Blockers {
        let mut b = Blockers::new(consts::ARENA_RADIUS + 20.0, 6.0);
        // the survey's "walkable" must count cliffs, or it reports a summit
        // as 100% walkable
        if let Some(f) = crate::map::floor() {
            b.set_floor(f);
        }
        for p in &self.placements {
            b.push_placed(p.kind, p.x, p.z, 0.0, p.quarter, p.scale);
        }
        b
    }

    /// A digest of a disc: what a model gets instead of eyes.
    pub fn survey(&self, x: f32, z: f32, radius: f32) -> String {
        let radius = radius.clamp(2.0, 60.0);
        let b = self.blockers();
        let mut counts: Vec<(String, usize)> = Vec::new();
        for p in self.placements.iter().filter(|p| (p.x - x).hypot(p.z - z) <= radius) {
            let name = format!("{:?}", p.kind);
            match counts.iter_mut().find(|c| c.0 == name) {
                Some(c) => c.1 += 1,
                None => counts.push((name, 1)),
            }
        }
        let (mut cells, mut open, mut hidden) = (0, 0, 0);
        let step = (radius / 12.0).max(0.75);
        let mut sx = -radius;
        while sx <= radius {
            let mut sz = -radius;
            while sz <= radius {
                if sx.hypot(sz) <= radius {
                    cells += 1;
                    open += !b.blocked(x + sx, z + sz, consts::GOD_RADIUS) as usize;
                    hidden += (b.concealment_at(x + sx, z + sz) > 0.3) as usize;
                }
                sz += step;
            }
            sx += step;
        }
        let pct = |n: usize| n * 100 / cells.max(1);
        let perches = b.with_perch().filter(|(k, p)| *p != crate::blockers::Perch::None && { let (ax, az, _) = k.shape.nearest_axis(x, z); (ax - x).hypot(az - z) <= radius }).count();
        let what = if counts.is_empty() { "nothing placed".to_string() } else { counts.iter().map(|c| format!("{} {}", c.1, c.0)).collect::<Vec<_>>().join(", ") };
        format!(
            "within {radius:.0} of ({x:.0}, {z:.0}): {what}. {}% walkable, {}% in cover, {perches} perch{}. nearest lane {:.0} u, river {:.0} u{}. (scatter is added later and is not counted.)",
            pct(open), pct(hidden), if perches == 1 { "" } else { "es" }, consts::lane_clearance(x, z), consts::river_dist(x, z),
            if consts::in_tower_clearing(x, z) { ", INSIDE a tower's clearing" } else { "" }
        )
    }

    /// Whole-map checks a single placement cannot make.
    pub fn validate(&self) -> Vec<String> {
        let mut out = Vec::new();
        for p in &self.placements {
            let (mx, mz) = mirror(p.x, p.z);
            let twin = self.placements.iter().any(|q| q.kind == p.kind && (q.x - mx).abs() < 1e-3 && (q.z - mz).abs() < 1e-3);
            if !twin {
                out.push(format!("{:?} at ({:.1}, {:.1}) has no mirror: the map favours one base", p.kind, p.x, p.z));
            }
        }
        let b = self.blockers();
        for lane in [consts::Lane::Top, consts::Lane::Mid, consts::Lane::Bot] {
            let pts = lane.polyline();
            let blocked = pts.windows(2).flat_map(|w| (0..=40).map(move |i| w[0] + (w[1] - w[0]) * (i as f32 / 40.0))).filter(|p| b.blocked(p.x, p.z, consts::GOD_RADIUS)).count();
            if blocked > 0 {
                out.push(format!("{lane:?} lane is blocked at {blocked} sampled points"));
            }
        }
        out
    }

    /// One line of the protocol. Never panics; an error is a sentence.
    pub fn call(&mut self, line: &str) -> Result<String, String> {
        let f: Vec<&str> = line.split_whitespace().collect();
        let num = |i: usize, what: &str| f.get(i).and_then(|s| s.parse::<f32>().ok()).filter(|v| v.is_finite()).ok_or(format!("{what} must be a number"));
        match f.first().copied() {
            Some("place") => {
                let name = f.get(1).ok_or("place what?")?;
                let kind = KINDS.iter().find(|k| k.0.eq_ignore_ascii_case(name)).map(|k| k.1).ok_or_else(|| format!("no such kind {name:?}; kinds are {}", KINDS.iter().map(|k| k.0).collect::<Vec<_>>().join(", ")))?;
                let q = f.get(4).and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
                self.place(kind, num(2, "x")?, num(3, "z")?, q)
            }
            Some("survey") => Ok(self.survey(num(1, "x")?, num(2, "z")?, num(3, "radius")?)),
            Some("undo") => self.undo(),
            Some("validate") => {
                let v = self.validate();
                if v.is_empty() { Ok(format!("valid: {} props, mirrored, all three lanes clear", self.placements.len())) } else { Err(v.join("; ")) }
            }
            Some(other) => Err(format!("no such tool {other:?}; tools are place, survey, undo, validate, done")),
            None => Err("an empty line is not a call".into()),
        }
    }
}

/// Whoever is authoring: a script, a person, a model.
pub trait MapAuthor {
    /// Given the transcript so far, the next call -- or `None` to stop.
    fn next(&mut self, transcript: &str) -> Option<String>;
}

/// Run an author against a draft. The transcript IS the interface: every
/// call and what came back, in order, so the loop can be read and steered.
pub fn run(draft: &mut MapDraft, author: &mut dyn MapAuthor) -> String {
    let mut t = String::new();
    for _ in 0..MAX_STEPS {
        let Some(line) = author.next(&t) else { break };
        let line = line.lines().next().unwrap_or("").trim().to_string();
        if line == "done" {
            t.push_str("> done\n");
            break;
        }
        let res = draft.call(&line);
        t.push_str(&format!("> {line}\n{}\n", match res { Ok(s) => format!("ok: {s}"), Err(e) => format!("REFUSED: {e}") }));
    }
    t
}

// ─────────────────────────────────────────────────────────────────────────
// Driving a MODEL. Three things taken from unendlich's voxel-tools
// (`llm.rs`, `job.rs`), each of which it learned by being bitten:
//
// 1. The conversation is PAIRS, not a blob. `Completer::complete_chain`
//    takes `prior: &[(user, reply)]` so a provider with a KV cache prefills
//    only the new turn. `MapAuthor::next(&transcript)` above re-sends
//    everything every turn; fine for a script, ruinous for `ai-gpu`.
// 2. A model's reply is not a call. It is prose, a code fence, several calls,
//    and sometimes half a call where the token budget ran out.
// 3. Multi-turn building needs a PLAN with checkable goals and a readback of
//    what is still unmet, or the model wanders and nothing says when to stop.

/// unendlich's own completer and turn control, not copies of them. I first
/// wrote a `ChainCompleter` trait here "with the same shape" -- which meant
/// every provider unendlich already has (the command one, the HTTP one, the
/// in-process `ai-gpu` one) would have needed an adapter before it could
/// author a map. Taking the real trait means they all work today, and a turn
/// here can be CANCELLED and reports PROGRESS, which the copy could not.
pub use voxel_tools::llm::{Completer, TurnControl};

pub const MAX_CALLS_PER_REPLY: usize = 8;
pub const MAX_TURNS: usize = 40;
const TOOLS: [&str; 5] = ["place", "survey", "undo", "validate", "done"];

/// The calls in a reply, and whether it was cut off.
///
/// Prose and fences are skipped: only a line that STARTS with a tool name is
/// a call. A final line with no newline after it and too few arguments is a
/// reply that ran out of tokens mid-call; it is dropped and reported, because
/// executing half a call and explaining a parse error to the model are both
/// worse than saying "you were cut off".
pub fn extract_calls(reply: &str) -> (Vec<String>, bool) {
    let mut calls: Vec<String> = Vec::new();
    for raw in reply.lines() {
        let line = raw.trim().trim_start_matches(['>', '-', '*', '`']).trim();
        let first = line.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
        if TOOLS.contains(&first.as_str()) && calls.len() < MAX_CALLS_PER_REPLY {
            calls.push(line.trim_end_matches('`').trim().to_string());
        }
    }
    let arity = |c: &str| match c.split_whitespace().next().map(str::to_ascii_lowercase).as_deref() {
        Some("place") => 4,
        Some("survey") => 4,
        _ => 1,
    };
    let cut = !reply.ends_with('\n') && calls.last().is_some_and(|c| c.split_whitespace().count() < arity(c));
    if cut {
        calls.pop();
    }
    (calls, cut)
}

/// One thing the map must end up having, with a check a machine can run.
pub struct Goal {
    pub name: &'static str,
    pub want: &'static str,
    /// `Ok(how it stands)` when met, `Err(how far short)` when not.
    pub check: fn(&MapDraft) -> Result<String, String>,
}

/// A plan: the goals, in the order they should be worked.
pub struct Brief {
    pub title: &'static str,
    pub goals: Vec<Goal>,
}

impl Brief {
    /// The centre of the map: the same things `midjungle`'s tests demand of
    /// the Hollow I built by hand, stated as goals an author can be given.
    pub fn hollow() -> Brief {
        const R: f32 = crate::midjungle::HOLLOW_RADIUS;
        fn near(d: &MapDraft) -> impl Iterator<Item = &Placement> {
            d.placements.iter().filter(|p| p.x.hypot(p.z) < R)
        }
        Brief {
            title: "the Hollow: a furnished centre within 21 u of the origin",
            goals: vec![
                Goal { name: "perches", want: "at least 6 things to leap onto, at 3 or more heights", check: |d| {
                    let tops: Vec<i32> = near(d).filter(|p| p.kind.perch() != crate::blockers::Perch::None).map(|p| (p.kind.footprint().top * 2.0) as i32).collect();
                    let kinds = tops.iter().collect::<std::collections::BTreeSet<_>>().len();
                    if tops.len() >= 6 && kinds >= 3 { Ok(format!("{} perches at {kinds} heights", tops.len())) } else { Err(format!("{} of 6 perches, {kinds} of 3 heights", tops.len())) }
                } },
                Goal { name: "cover", want: "at least 8 fern or bush beds to stalk through", check: |d| {
                    let n = near(d).filter(|p| matches!(p.kind, PropKind::Fern | PropKind::Bush)).count();
                    if n >= 8 { Ok(format!("{n} beds")) } else { Err(format!("{n} of 8 beds")) }
                } },
                Goal { name: "room", want: "still at least 72% walkable: furnished, not filled", check: |d| {
                    let b = d.blockers();
                    let (mut open, mut cells) = (0, 0);
                    let mut x = -R;
                    while x < R {
                        let mut z = -R;
                        while z < R {
                            if x.hypot(z) < R {
                                cells += 1;
                                open += !b.blocked(x, z, consts::GOD_RADIUS) as usize;
                            }
                            z += 1.5;
                        }
                        x += 1.5;
                    }
                    let pct = open * 100 / cells.max(1);
                    if pct >= 72 { Ok(format!("{pct}% walkable")) } else { Err(format!("only {pct}% walkable: take something out")) }
                } },
            ],
        }
    }

    /// What is done and what is not, for the top of every turn. This is what
    /// keeps a long build pointed somewhere (voxel-tools' `Job::readback`).
    pub fn readback(&self, d: &MapDraft) -> String {
        let mut out = format!("BRIEF: {}\n", self.title);
        for (i, g) in self.goals.iter().enumerate() {
            match (g.check)(d) {
                Ok(how) => out.push_str(&format!("  {}. [done] {} -- {how}\n", i + 1, g.name)),
                Err(short) => out.push_str(&format!("  {}. [TODO] {}: {} -- {short}\n", i + 1, g.name, g.want)),
            }
        }
        out
    }

    pub fn met(&self, d: &MapDraft) -> bool {
        self.goals.iter().all(|g| (g.check)(d).is_ok())
    }
}

/// Drive a model through a brief. Returns the transcript.
///
/// Each turn the model is sent the readback and the last results, and may
/// answer with several calls. They run in order and STOP AT THE FIRST
/// REFUSAL: later calls in the same reply were written assuming the earlier
/// ones worked, and building on a placement that never happened is how a
/// structure goes wrong three steps before anyone notices.
pub fn run_chain(draft: &mut MapDraft, brief: &Brief, model: &mut dyn Completer, ctl: &TurnControl) -> String {
    let system = format!("{}\n\n{}", tools(), "Reply with tool calls only, one per line, at most 8. Work the TODO goals in order. Say `done` when every goal is done.");
    let mut prior: Vec<(String, String)> = Vec::new();
    let mut transcript = String::new();
    let mut last = String::from("(nothing yet)");
    for turn in 0..MAX_TURNS {
        if ctl.cancelled() {
            transcript.push_str(&format!("-- cancelled before turn {turn}; what was built is kept\n"));
            break;
        }
        let user = format!("{}LAST RESULTS:\n{last}", brief.readback(draft));
        let reply = match model.complete_chain(&system, &prior, &user, ctl) {
            Ok(r) => r,
            Err(e) => {
                transcript.push_str(&format!("-- turn {turn}: the model failed: {e}\n"));
                break;
            }
        };
        // NOT voxel_tools::llm::strip_fences. It looks generic and is not: with
        // no fence present it keeps only lines starting with unendlich's own
        // build keywords (asset, frame, part, repeat...), so every `place` and
        // `survey` was filtered out as not-a-program and the loop ran 40 turns
        // of "no tool call found". The COMPLETER and TURN CONTROL are
        // protocol-neutral and are reused; reply cleaning belongs to each
        // grammar. `extract_calls` handles fences for this one.
        let (calls, cut) = extract_calls(&reply);
        let mut results = String::new();
        let mut finished = false;
        for c in &calls {
            if c.eq_ignore_ascii_case("done") {
                finished = true;
                break;
            }
            match draft.call(c) {
                Ok(s) => results.push_str(&format!("> {c}\nok: {s}\n")),
                Err(e) => {
                    results.push_str(&format!("> {c}\nREFUSED: {e}\n(the rest of that reply was not run)\n"));
                    break;
                }
            }
        }
        if calls.is_empty() {
            results.push_str("no tool call found in your reply; reply with lines like `place Rock -9 3.5`\n");
        }
        if cut {
            results.push_str("your reply was cut off mid-call; the unfinished call was NOT run. Send fewer calls per reply.\n");
        }
        ctl.report(draft.placements.len(), results.lines().last().unwrap_or(""));
        transcript.push_str(&format!("-- turn {turn}\n{results}"));
        prior.push((user, reply));
        last = results;
        if finished || (brief.met(draft) && draft.validate().is_empty()) {
            transcript.push_str(&format!("-- finished after {} turns: {}\n", turn + 1, if brief.met(draft) { "every goal met" } else { "the author said done with goals unmet" }));
            break;
        }
    }
    transcript.push_str(&brief.readback(draft));
    transcript
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Script(Vec<&'static str>, usize);
    impl MapAuthor for Script {
        fn next(&mut self, _: &str) -> Option<String> {
            self.1 += 1;
            self.0.get(self.1 - 1).map(|s| s.to_string())
        }
    }

    /// An author building an outcrop, making every kind of mistake on the
    /// way. Each refusal must be a reason it could act on, nothing may
    /// half-apply, and the result must be fair without the author trying.
    #[test]
    fn an_author_is_refused_in_words_and_cannot_make_an_unfair_map() {
        let mut d = MapDraft::default();
        let t = run(&mut d, &mut Script(vec![
            "survey -40 30 12",
            "place Rock -40 30",
            "place BoulderCluster -44 35 1",
            "place Rock -44 35.5",          // on top of the boulders
            "place Rock 0 40",              // on mid lane
            "place Bush 12 54",             // off the lane, but at a tower's feet
            "place Dragon 10 10",           // not a thing
            "place Rock 900 0",             // off the map
            "place Rock banana 3",
            "teleport 1 2",
            "place Fern -42 27",
            "undo",
            "survey -40 30 12",
            "validate",
            "done",
        ], 0));
        println!("{t}");
        for want in ["overlaps the BoulderCluster", "from a lane's centreline", "tower's cleared ground", "no such kind \"Dragon\"", "outside the arena", "x must be a number", "no such tool \"teleport\"", "took back the Fern", "valid: 4 props"] {
            assert!(t.contains(want), "the transcript never says {want:?}");
        }
        assert_eq!(t.matches("REFUSED").count(), 7, "seven mistakes, seven refusals");
        assert_eq!(d.placements.len(), 4, "two placements and their mirrors; the fern was undone; nothing refused left a trace");
        assert!(d.validate().is_empty());
        assert!(t.contains("1 Rock, 1 BoulderCluster") && t.contains("2 perches"), "the survey reports what is there");
        // The mirror is refused too, as a unit: a spot fine here whose twin is not.
        let mut e = MapDraft::default();
        e.placements.push(Placement { kind: PropKind::Rock, x: 30.0, z: -20.0, quarter: 0, scale: 1.0 });
        let err = e.place(PropKind::Rock, -30.0, 20.5, 0).unwrap_err();
        assert!(err.contains("its mirror") && e.placements.len() == 1, "{err}");
    }

    /// The rules apply to me. The centre I authored by hand, replayed
    /// through the same door an AI uses.
    #[test]
    fn my_own_hand_authored_centre_passes_the_rules_an_ai_is_held_to() {
        let mut d = MapDraft::default();
        let mut refused = Vec::new();
        for p in crate::midjungle::layout().into_iter().filter(|p| p.z > 0.0 || (p.z == 0.0 && p.x > 0.0)) {
            if let Err(why) = d.place(p.kind, p.x, p.z, p.quarter) {
                refused.push(format!("{:?} at ({:.1}, {:.1}): {why}", p.kind, p.x, p.z));
            }
        }
        assert!(refused.is_empty(), "my layout breaks {} of the rules I wrote for an AI:\n{}", refused.len(), refused.join("\n"));
        assert!(d.validate().is_empty(), "{:?}", d.validate());
        assert!(d.placements.len() > 40);
    }

    #[test]
    fn a_reply_is_prose_fences_several_calls_and_sometimes_half_a_call() {
        let (calls, cut) = extract_calls("Sure! I'll start with the rocks.\n```\nplace Rock -9 3.5\n- place Fern 10 2.5 0\n```\nThen I will survey the area.\nsurvey 0 0 20\n");
        assert_eq!(calls, vec!["place Rock -9 3.5", "place Fern 10 2.5 0", "survey 0 0 20"], "prose mentioning a tool mid-sentence is not a call");
        assert!(!cut);
        let (calls, cut) = extract_calls("place Rock -9 3.5\nplace BoulderCluster -13");
        assert_eq!((calls, cut), (vec!["place Rock -9 3.5".to_string()], true), "out of tokens mid-call: dropped, and said so");
        let (calls, cut) = extract_calls("place Rock -9 3.5\nundo");
        assert_eq!((calls.len(), cut), (2, false), "a complete last line with no newline is fine");
        let many = "place Fern 1 9\n".repeat(30);
        assert_eq!(extract_calls(&many).0.len(), MAX_CALLS_PER_REPLY);
    }

    /// The whole loop, against a canned model that behaves like a real one:
    /// chatty, batches calls, makes a mistake mid-batch, gets cut off once.
    #[test]
    fn a_model_is_driven_through_a_brief_turn_by_turn() {
        // unendlich's OWN test double: if this drives the loop, so does every
        // real provider behind the same trait.
        let mut model = voxel_tools::llm::FakeCompleter::new(vec![
            "Let me look first.\nsurvey 0 0 21\n",
            // the third call is on the lane; the fourth must NOT run
            "```\nplace Rock -9 3.5\nplace BoulderCluster -13.5 9.5\nplace RuinPillar 0 15\nplace StoneHead 14 14\n```\n",
            "place RuinPillar -9.5 15.5\nplace Fern 10 2.5\nplace Fern 16.5 3.5\nplace Fern -15.5 3\nplace Fern -7.5 10\nplace Bush",
            "done",
        ].into_iter().map(String::from).collect());
        let mut d = MapDraft::default();
        let brief = Brief::hollow();
        assert!(brief.readback(&d).contains("[TODO] perches") && brief.readback(&d).contains("0 of 6 perches"));
        let ctl = TurnControl::default();
        let t = run_chain(&mut d, &brief, &mut model, &ctl);
        println!("{t}");
        // Three turns, not four: the loop ends ITSELF once every goal is met and
        // the map validates, without waiting for the model to say `done`. (I
        // first asserted four here, as if it would not.)
        assert_eq!(model.calls(), 3, "three turns");
        assert_eq!(ctl.snapshot().0, d.placements.len(), "progress was reported as it went: a UI can show it");
        assert!(t.contains("from a lane's centreline") && t.contains("the rest of that reply was not run"));
        assert!(!d.placements.iter().any(|p| p.kind == PropKind::StoneHead), "the call after a refusal did not run");
        assert!(t.contains("cut off mid-call") && !d.placements.iter().any(|p| p.kind == PropKind::Bush), "half a call is not run");
        assert!(t.contains("finished after 3 turns: every goal met"), "{t}");
        assert!(t.contains("[done] perches -- 6 perches at 3 heights"), "three placements and their mirrors: {t}");
        assert!(t.contains("[done] cover -- 8 beds") && t.contains("[done] room"));
        // A model that dies is a line in the transcript, not a panic.
        let t = run_chain(&mut MapDraft::default(), &brief, &mut voxel_tools::llm::DeadCompleter("no GPU".into()), &TurnControl::default());
        assert!(t.contains("the model failed") && t.contains("no GPU"), "{t}");
        // Cancelled before it starts: the model is never called.
        let stop = TurnControl::default();
        stop.cancel();
        let mut never = voxel_tools::llm::FakeCompleter::new(vec!["place Rock -9 3.5".into()]);
        let t = run_chain(&mut MapDraft::default(), &brief, &mut never, &stop);
        assert!(t.contains("cancelled before turn 0") && never.calls() == 0, "{t}");
    }

    /// The brief is not easier than what I asked of myself: the Hollow I
    /// built by hand meets it.
    #[test]
    fn the_hand_built_hollow_meets_the_brief_given_to_a_model() {
        let mut d = MapDraft::default();
        d.placements = crate::midjungle::layout();
        let brief = Brief::hollow();
        assert!(brief.met(&d), "{}", brief.readback(&d));
    }

    #[test]
    fn the_loop_is_bounded_and_garbage_is_a_sentence() {
        struct Forever;
        impl MapAuthor for Forever {
            fn next(&mut self, _: &str) -> Option<String> { Some("place Fern 60 60".into()) }
        }
        let mut d = MapDraft::default();
        let t = run(&mut d, &mut Forever);
        assert_eq!(t.matches("\n> ").count() + 1, MAX_STEPS, "an author that never stops is stopped");
        assert!(d.placements.len() <= MAX_PLACEMENTS);
        for junk in ["", "   ", "place", "place Rock", "survey nan 0 5", "place Rock inf 0", "undo undo undo"] {
            let _ = MapDraft::default().call(junk); // must not panic
        }
        assert!(tools().contains("survey") && tools().contains("StoneHead"));
    }

    /// On Twinfang Pass the engine knows where the rock is: a placement on
    /// the summit, the plateau or the gorge wall is refused in words, its
    /// survey counts cliff as unwalkable, and twins mirror across z = 0.
    /// (Before: the summit rock was ACCEPTED and validate() passed it.)
    #[test]
    fn on_the_pass_the_rock_is_refused_and_twins_mirror_in_z() {
        crate::map::with(crate::map::MapId::Pass, || {
            let mut d = MapDraft::default();
            let k = crate::pass::S;
            for (x, z) in [(-60.0 * k, 0.0), (24.0 * k, -60.0 * k), (-14.0 * k, -60.0 * k)] {
                let e = d.place(PropKind::Rock, x, z, 0).unwrap_err();
                assert!(e.contains("in the rock"), "({x},{z}): {e}");
            }
            assert!(d.survey(-60.0 * k, 0.0, 8.0).contains("0% walkable"), "{}", d.survey(-60.0 * k, 0.0, 8.0));
            // a floor spot: its twin is (x, -z)
            d.place(PropKind::Bush, -26.0 * k, -38.0 * k, 0).unwrap();
            assert!(d.placements.iter().any(|p| (p.x + 26.0 * k).abs() < 1e-3 && (p.z - 38.0 * k).abs() < 1e-3), "{:?}", d.placements);
            assert!(d.validate().is_empty(), "{:?}", d.validate());
            assert!(tools().contains("Twinfang Pass") && tools().contains("across z = 0"));
        });
        // and the jungle is as it was: through the origin
        assert_eq!(mirror(3.0, 4.0), (-3.0, -4.0));
    }
}
