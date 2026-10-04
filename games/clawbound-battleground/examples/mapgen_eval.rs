// Would the map creation engine (`mapgen`) have made Twinfang Pass as well
// as the hand author (`pass` + `passworld`)?
//
//   cargo run --example mapgen_eval probe          guard rails on the pass
//   cargo run --example mapgen_eval run <calls>    drive the engine with a
//                                                  file of protocol lines,
//                                                  print the transcript
//   cargo run --example mapgen_eval score <calls>  the same, then score the
//                                                  engine's map and the hand
//                                                  author's with ONE scorer
//
// The scorer was written before any engine authoring was done, from the
// brief the pass was built to: bleak and sparse, nothing in the rock, lanes
// clear, fair, scrub and grass in the clearings without filling them, scree
// at the feet of the cliffs, a few pines, waterfalls. It scores only what
// both can express (props on the floor) plus, separately, what only one can.

use clawbound::blockers::{Blockers, PropKind};
use clawbound::consts::{self, Lane, Team};
use clawbound::map::{self, MapId};
use clawbound::mapgen::{self, MapAuthor, MapDraft};
use clawbound::pass;
use clawbound::passworld::{self, Item};

struct Lines(Vec<String>, usize);
impl MapAuthor for Lines {
    fn next(&mut self, _t: &str) -> Option<String> {
        self.1 += 1;
        self.0.get(self.1 - 1).cloned()
    }
}

#[derive(Clone, Copy)]
struct P {
    kind: PropKind,
    x: f32,
    z: f32,
}

fn solid(k: PropKind) -> bool {
    matches!(k, PropKind::Rock | PropKind::BoulderCluster | PropKind::Pine | PropKind::RuinPillar | PropKind::StoneHead | PropKind::JungleTree | PropKind::StranglerFig | PropKind::Palm | PropKind::FallenLog)
}

fn blockers_of(ps: &[P]) -> Blockers {
    let mut b = Blockers::new(consts::ARENA_RADIUS + 20.0, 6.0);
    b.set_floor(pass::floor());
    for p in ps {
        b.push_prop(p.kind, p.x, p.z, 0.0, 1.0);
    }
    b
}

/// One scorer for both. Every line is a measure the brief asked for.
fn score(name: &str, ps: &[P], extra_pines: usize, falls: usize) {
    let f = pass::floor();
    let n = ps.len().max(1);
    let in_rock = ps.iter().filter(|p| f.sdf(p.x, p.z) > -0.3).count();
    let on_lane = ps.iter().filter(|p| solid(p.kind) && consts::lane_clearance(p.x, p.z) < 4.0).count();
    let twin = |a: &P, mx: f32, mz: f32| ps.iter().any(|q| q.kind == a.kind && (q.x - mx).abs() < 0.05 && (q.z - mz).abs() < 0.05);
    let zfair = ps.iter().filter(|p| p.z.abs() < 0.05 || twin(p, p.x, -p.z)).count();
    let pfair = ps.iter().filter(|p| twin(p, -p.x, -p.z)).count();
    let b = blockers_of(ps);
    // lanes: sampled like mapgen::validate
    let mut lane_blocked = 0;
    for lane in [Lane::Top, Lane::Mid, Lane::Bot] {
        let pts = lane.polyline();
        for w in pts.windows(2) {
            for i in 0..=40 {
                let q = w[0] + (w[1] - w[0]) * (i as f32 / 40.0);
                lane_blocked += b.blocked(q.x, q.z, consts::GOD_RADIUS) as usize;
            }
        }
    }
    // clearings (not the boss grounds): scrub/grass, and still walkable
    let (mut furnished, mut cl, mut walk_sum) = (0, 0, 0.0f32);
    for (cx, cz, r) in pass::clearings() {
        if clawbound::bosses::in_pit(cx, cz) {
            continue;
        }
        cl += 1;
        let green = ps.iter().filter(|p| matches!(p.kind, PropKind::Fern | PropKind::Bush) && (p.x - cx).hypot(p.z - cz) < r).count();
        furnished += (green >= 3) as usize;
        let (mut open, mut cells) = (0, 0);
        let mut x = -r;
        while x <= r {
            let mut z = -r;
            while z <= r {
                if x.hypot(z) < r - 1.0 {
                    cells += 1;
                    open += !b.blocked(cx + x, cz + z, consts::GOD_RADIUS) as usize;
                }
                z += 1.0;
            }
            x += 1.0;
        }
        walk_sum += open as f32 / cells.max(1) as f32;
    }
    let stones: Vec<&P> = ps.iter().filter(|p| matches!(p.kind, PropKind::Rock | PropKind::BoulderCluster)).collect();
    let scree = stones.iter().filter(|p| (-3.4..=0.0).contains(&f.sdf(p.x, p.z))).count();
    let pines = ps.iter().filter(|p| p.kind == PropKind::Pine).count();
    let kinds: std::collections::BTreeMap<String, usize> = ps.iter().fold(Default::default(), |mut m, p| {
        *m.entry(format!("{:?}", p.kind)).or_default() += 1;
        m
    });
    let jungle: usize = ps.iter().filter(|p| matches!(p.kind, PropKind::JungleTree | PropKind::StranglerFig | PropKind::Palm | PropKind::RuinPillar | PropKind::StoneHead | PropKind::FallenLog)).count();
    println!("== {name}: {} props on the floor", ps.len());
    println!("  kinds: {kinds:?}");
    println!("  in the rock (floor sdf > -0.3):        {in_rock} ({:.0}%)", 100.0 * in_rock as f32 / n as f32);
    println!("  solid props on a lane (< 4 u):         {on_lane}");
    println!("  lane samples blocked:                  {lane_blocked}");
    println!("  fair, the pass's way (z mirror):       {:.0}%", 100.0 * zfair as f32 / n as f32);
    println!("  fair, the engine's way (point mirror): {:.0}%", 100.0 * pfair as f32 / n as f32);
    println!("  clearings with >= 3 scrub/grass:       {furnished} of {cl}");
    println!("  clearings' mean walkable fraction:     {:.0}%", 100.0 * walk_sum / cl.max(1) as f32);
    println!("  stones that are scree (at a cliff foot): {scree} of {}", stones.len());
    println!("  pines on the floor: {pines}   pines on the plateau: {extra_pines}   waterfalls: {falls}");
    println!("  jungle kinds on a mountain pass:       {jungle}");
}

fn hand() -> (Vec<P>, usize, usize) {
    let (mut ps, mut hp, mut fa, mut scenery) = (Vec::new(), 0, 0, 0usize);
    for it in passworld::placements() {
        match it {
            Item::Prop { kind, x, z, .. } => ps.push(P { kind, x, z }),
            Item::HighPine { .. } => hp += 1,
            Item::Fall { .. } => fa += 1,
            // Rooted in the rock, by passworld's own account: none of them is
            // a floor prop a lane or a clearing can be scored against, so
            // they are counted and said, not folded into the prop list.
            Item::Crag { .. } | Item::Gem { .. } | Item::Arch { .. }
            | Item::Statue { .. } | Item::Burrow { .. } | Item::Diorama { .. } => scenery += 1,
        }
    }
    if scenery > 0 {
        println!("  (hand placements also carry {scenery} rock-rooted scenery items -- crags, gems, arches, statues, burrows, dioramas -- outside this score)");
    }
    (ps, hp, fa)
}

fn drive(path: &str) -> (MapDraft, String) {
    let lines: Vec<String> = std::fs::read_to_string(path)
        .expect("calls file")
        .lines()
        .map(|l| l.split('#').next().unwrap_or("").trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    let mut d = MapDraft::default();
    let t = mapgen::run(&mut d, &mut Lines(lines, 0));
    (d, t)
}

fn main() {
    map::set_active(MapId::Pass);
    let mode = std::env::args().nth(1).unwrap_or_default();
    match mode.as_str() {
        "probe" => {
            let mut d = MapDraft::default();
            println!("-- does the engine know where the rock is?");
            for (what, x, z) in [("the west summit", -60.0, 0.0), ("the plateau between mid and the east shoulder", 24.0, -60.0), ("inside the gorge wall", -14.0, -60.0), ("a clearing (floor)", -150.0, -40.0)] {
                let r = d.call(&format!("place Rock {x} {z}"));
                println!("  place Rock at {what} ({x}, {z}): {}  [floor sdf {:.1}]", match &r { Ok(s) => format!("ACCEPTED ({s})"), Err(e) => format!("refused: {e}") }, pass::floor().sdf(x, z));
                if r.is_ok() {
                    let _ = d.call("undo");
                }
            }
            println!("-- the engine's fairness is a point mirror; the pass's is a z mirror:");
            for (x, z) in [(-155.0, -43.0), (-110.0, -120.0), (-26.0, -38.0)] {
                println!("  floor at ({x},{z}): sdf {:.1}; its point mirror ({},{}): sdf {:.1}; its z mirror ({},{}): sdf {:.1}", pass::floor().sdf(x, z), -x, -z, pass::floor().sdf(-x, -z), x, -z, pass::floor().sdf(x, -z));
            }
            println!("-- what the engine can place: {:?}", mapgen::KINDS.iter().map(|k| k.0).collect::<Vec<_>>());
            println!("-- what a survey says about the west summit: {}", d.survey(-60.0, 0.0, 10.0));
            println!("-- validate() of a draft with a rock on the summit:");
            let _ = d.call("place Rock -60 0");
            println!("  {:?}", d.validate());
            let t = Team::Blue;
            let _ = t;
        }
        "run" => {
            let path = std::env::args().nth(2).expect("calls file");
            let (d, t) = drive(&path);
            print!("{t}");
            println!("-- {} placements; validate: {:?}", d.placements.len(), d.validate());
        }
        "score" => {
            let path = std::env::args().nth(2).expect("calls file");
            let (d, t) = drive(&path);
            let refused = t.matches("REFUSED").count();
            let calls = t.matches("\n> ").count() + t.starts_with("> ") as usize;
            println!("engine run: {calls} calls, {refused} refused, validate: {:?}", d.validate());
            let eng: Vec<P> = d.placements.iter().map(|p| P { kind: p.kind, x: p.x, z: p.z }).collect();
            score("ENGINE (mapgen, driven by hand through its protocol)", &eng, 0, 0);
            let (h, hp, fa) = hand();
            score("HAND (passworld::placements)", &h, hp, fa);
        }
        _ => eprintln!("usage: mapgen_eval probe | run <calls> | score <calls>"),
    }
}
