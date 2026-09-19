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

/// The tool descriptions, for a system prompt.
pub fn tools() -> String {
    format!(
        "You author a jungle arena by calling tools, one per line. You cannot see the map; ask.\n\
         place <Kind> <x> <z> [quarter 0-3]  -- place a prop AND its mirror through the origin. Kinds: {}.\n\
         survey <x> <z> <radius>             -- a digest of what is there.\n\
         undo                                -- take back the last placement (both halves).\n\
         validate                            -- check the whole map.\n\
         done                                -- finish.\n\
         Units are world units; the arena is {:.0} across; +z is the red base. Refusals say why.",
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
        if x.hypot(z) > consts::ARENA_RADIUS - 4.0 {
            return Some(format!("outside the arena (radius {:.0})", consts::ARENA_RADIUS));
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
        let own_twin = x.hypot(z) < kind.footprint().radius.max(1.0);
        let before = self.placements.len();
        self.placements.push(Placement { kind, x, z, quarter: quarter % 4, scale: 1.0 });
        if !own_twin {
            if let Some(why) = self.refusal(kind, -x, -z) {
                self.placements.truncate(before);
                return Err(format!("its mirror at ({:.1}, {:.1}) is refused: {why}", -x, -z));
            }
            self.placements.push(Placement { kind, x: -x, z: -z, quarter: (quarter + 2) % 4, scale: 1.0 });
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
            let twin = self.placements.iter().any(|q| q.kind == p.kind && (q.x + p.x).abs() < 1e-3 && (q.z + p.z).abs() < 1e-3);
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
}
