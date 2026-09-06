//! Places, not operations.
//!
//! The corpus does not say "carve a canyon". It says "dig out a grand
//! canyon for the mesa biome, add some sand, plant a few cacti" -- a
//! PLACE, made of several carves and layers that belong together. Six
//! IR lines can say that, but only if the model knows which six, and
//! the thing it actually knows is the name of the place.
//!
//! So a feature is a named composition: `feature badlands @0,0 r=140`
//! is a mesa, three side canyons off it, a dry wash between them, a
//! desert layer and a scatter of ruins. One word, a hundred thousand
//! cells, and the parts are the ops that already exist.
//!
//! STREAMED, NOT BATCHED
//!
//! A feature is seconds of work and hundreds of thousands of cells. It
//! is generated PART BY PART: `plan()` returns the parts, and the host
//! runs them one at a time -- so the first canyon appears while the
//! third is still being computed, the hive gets its cells in chunks
//! the size of one carve rather than one enormous list, and the
//! marker's blob grows as the place takes shape. A feature that
//! produced one change list would have held the drone for four seconds
//! and then handed the hive a million cells at once.
//!
//! Each part is IR, so the whole feature previews, commits, undoes and
//! journals like anything else.

use crate::ambient::Ambient;

/// One step of a feature: a line of IR and what it is for.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub what: String,
    pub ir: String,
    /// Rough cells, for the progress the marker shows.
    pub estimate: i64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FeatureArgs {
    pub at: (i32, i32),
    /// The feature's scale in blocks: its radius or half-length.
    pub radius: i32,
    pub seed: u64,
    /// The ground at the anchor, for features that need a height.
    pub ground: i32,
}

pub struct Feature {
    pub name: &'static str,
    pub summary: &'static str,
    pub plan: fn(&FeatureArgs) -> Vec<Part>,
}

pub const FEATURES: &[Feature] = &[
    Feature { name: "badlands", summary: "a mesa, side canyons, a dry wash, desert cover and ruins",
              plan: badlands },
    Feature { name: "gorge", summary: "a canyon with a river in it, waterfalls into it, caves in its walls",
              plan: gorge },
    Feature { name: "caldera", summary: "a ring wall around a sunken floor, with a lake and vents",
              plan: caldera },
    Feature { name: "cliffs", summary: "a coastal cliff line with overhangs, sea caves and a beach",
              plan: cliffs },
    Feature { name: "highlands", summary: "raised ground cut by ravines, with tarns and a treeline",
              plan: highlands },
    Feature { name: "sinkhole", summary: "a collapsed cavern open to the sky, with a pool and a way down",
              plan: sinkhole },
];

pub fn lookup(name: &str) -> Option<&'static Feature> {
    FEATURES.iter().find(|f| f.name.eq_ignore_ascii_case(name.trim()))
}

pub fn vocabulary() -> String {
    FEATURES.iter().map(|f| format!("  {:<10} {}", f.name, f.summary)).collect::<Vec<_>>().join("\n")
}

fn hash(a: i64, s: u64) -> u64 {
    let mut h = (a as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ s;
    h ^= h >> 31; h = h.wrapping_mul(0x7FB5_D329_728E_A185); h ^ (h >> 27)
}
fn pick(a: i64, s: u64, lo: i32, hi: i32) -> i32 { lo + (hash(a, s) % (hi - lo + 1).max(1) as u64) as i32 }
/// A point on a circle of radius r about the centre.
fn around(c: (i32, i32), r: i32, deg: i32) -> (i32, i32) {
    let a = (deg as f32).to_radians();
    (c.0 + (r as f32 * a.cos()) as i32, c.1 + (r as f32 * a.sin()) as i32)
}

fn badlands(a: &FeatureArgs) -> Vec<Part> {
    let r = a.radius.clamp(40, 400);
    let mut v = Vec::new();
    // the mesa first: everything else is cut into what it leaves
    v.push(Part { what: "the mesa".into(),
                  ir: format!("mesa {},{} r={} drop={} seed={}\n", a.at.0, a.at.1, r / 3, r / 6, a.seed),
                  estimate: (r as i64 * 2).pow(2) * (r / 6) as i64 / 3 });
    // side canyons radiating out, at uneven angles
    for i in 0..3 {
        let deg = pick(i, a.seed, 0, 359) + i as i32 * 110;
        let from = around(a.at, r / 3, deg);
        let to = around(a.at, r, deg + pick(i + 90, a.seed, -25, 25));
        v.push(Part { what: format!("side canyon {}", i + 1),
                      ir: format!("canyon {},{} {},{} w={} depth={} floor=sand wall=sandstone seed={}\n",
                                  from.0, from.1, to.0, to.1, pick(i + 7, a.seed, 8, 18),
                                  pick(i + 13, a.seed, 10, 22), a.seed ^ i as u64),
                      estimate: r as i64 * 14 * 16 / 2 });
    }
    // a dry wash across the whole thing, shallow and wide
    let (w0, w1) = (around(a.at, r, 200), around(a.at, r, 20));
    v.push(Part { what: "the dry wash".into(),
                  ir: format!("canyon {},{} {},{} w={} depth=5 floor=sand seed={}\n",
                              w0.0, w0.1, w1.0, w1.1, r / 8, a.seed ^ 0x5a5a),
                  estimate: r as i64 * 2 * (r / 8) as i64 * 3 });
    v.push(Part { what: "desert cover".into(),
                  ir: format!("layer desert {},{} {},{} density=0.35\n",
                              a.at.0 - r, a.at.1 - r, a.at.0 + r, a.at.1 + r),
                  estimate: (r as i64 * 2).pow(2) / 12 });
    v.push(Part { what: "ruins on the mesa".into(),
                  ir: format!("scatter_json {{ \"structure\": \"house\", \"count\": 5, \"seed\": {},\n\
                               \"rail\": {{ \"kind\": \"area\", \"min\": [{}, {}], \"max\": [{}, {}], \"relative\": false }},\n\
                               \"rules\": {{ \"spacing\": {}, \"max_slope\": 2, \"footprint\": 5 }},\n\
                               \"params\": {{ \"style\": \"desert\", \"size\": \"small\" }} }}\n",
                              a.seed, a.at.0 - r / 4, a.at.1 - r / 4, a.at.0 + r / 4, a.at.1 + r / 4, r / 12),
                  estimate: 5 * 400 });
    v.push(Part { what: "weathering".into(),
                  ir: format!("weather {},{},{} {},{},{} ancient seed={}\n",
                              a.at.0 - r / 4, a.ground, a.at.1 - r / 4,
                              a.at.0 + r / 4, a.ground + 30, a.at.1 + r / 4, a.seed),
                  estimate: 2000 });
    v
}

fn gorge(a: &FeatureArgs) -> Vec<Part> {
    let r = a.radius.clamp(40, 400);
    let mut v = Vec::new();
    let (from, to) = (around(a.at, r, 190), around(a.at, r, 10));
    let depth = (r / 4).clamp(12, 60);
    v.push(Part { what: "the gorge".into(),
                  ir: format!("canyon {},{} {},{} w={} depth={} floor=stone seed={}\n",
                              from.0, from.1, to.0, to.1, (r / 6).clamp(10, 40), depth, a.seed),
                  estimate: r as i64 * 2 * (r / 6) as i64 * depth as i64 / 2 });
    // the river runs along its floor
    v.push(Part { what: "the river in it".into(),
                  ir: format!("river {},{} dir=0 len={}\n", from.0, from.1, r * 2),
                  estimate: r as i64 * 30 });
    // waterfalls off the rim, on alternating sides
    for i in 0..2 {
        let deg = if i == 0 { 100 } else { 260 };
        let p = around(a.at, r / 3, deg);
        v.push(Part { what: format!("waterfall {}", i + 1),
                      ir: format!("waterfall {},{} dir={} w={} h={} seed={}\n",
                                  p.0, p.1, (deg + 180) % 360, pick(i, a.seed, 4, 9), depth - 2, a.seed ^ i as u64),
                      estimate: 8 * depth as i64 * 6 });
    }
    // caves into the walls
    for i in 0..2 {
        let deg = if i == 0 { 80 } else { 280 };
        let p = around(a.at, r / 5, deg);
        v.push(Part { what: format!("cave in the wall {}", i + 1),
                      ir: format!("cave {},{} dir={} len={} r=4 branches=2 seed={}\n",
                                  p.0, p.1, deg, r, a.seed ^ (i as u64 + 40)),
                      estimate: r as i64 * 48 });
    }
    v
}

fn caldera(a: &FeatureArgs) -> Vec<Part> {
    let r = a.radius.clamp(30, 300);
    let mut v = Vec::new();
    // raise a ring wall, then drop the middle: a rim you look over
    v.push(Part { what: "the rim".into(),
                  ir: format!("brush raise {},{} by={} r={} mesa\n", a.at.0, a.at.1, (r / 4).clamp(6, 40), r),
                  estimate: (r as i64 * 2).pow(2) * 6 });
    v.push(Part { what: "the crater floor".into(),
                  ir: format!("brush lower {},{} by={} r={} smooth\n",
                              a.at.0, a.at.1, (r / 5).clamp(6, 40), (r * 2) / 3),
                  estimate: (r as i64).pow(2) * 6 });
    v.push(Part { what: "the lake".into(),
                  ir: format!("lake {},{} r={} level={}\n", a.at.0, a.at.1, r / 3, a.ground),
                  estimate: (r as i64 / 3).pow(2) * 3 });
    // vents: short caves down from the floor
    for i in 0..3 {
        let p = around(a.at, r / 2, pick(i, a.seed, 0, 359));
        v.push(Part { what: format!("vent {}", i + 1),
                      ir: format!("cave {},{} dir={} len=30 r=3 branches=1 seed={}\n",
                                  p.0, p.1, pick(i + 5, a.seed, 0, 359), a.seed ^ i as u64),
                      estimate: 30 * 27 });
    }
    v.push(Part { what: "a scorched floor".into(),
                  ir: format!("mood fiery {},{} {},{}\n", a.at.0 - r, a.at.1 - r, a.at.0 + r, a.at.1 + r),
                  estimate: 0 });
    v
}

fn cliffs(a: &FeatureArgs) -> Vec<Part> {
    let r = a.radius.clamp(40, 400);
    let mut v = Vec::new();
    // the cliff line runs north-south; the sea is west
    v.push(Part { what: "the cliff line".into(),
                  ir: format!("brush raise {},{} by={} r={} mesa\n",
                              a.at.0 + r / 3, a.at.1, (r / 4).clamp(10, 50), r),
                  estimate: (r as i64 * 2).pow(2) * 8 });
    v.push(Part { what: "the sea".into(),
                  ir: format!("brush lower {},{} by={} r={} smooth\n",
                              a.at.0 - r, a.at.1, (r / 5).clamp(8, 40), r),
                  estimate: (r as i64 * 2).pow(2) * 6 });
    // overhangs and sea caves along the face
    for i in 0..3 {
        let z = a.at.1 - r / 2 + i as i32 * (r / 2);
        v.push(Part { what: format!("overhang {}", i + 1),
                      ir: format!("overhang {},{} dir=180 w={} depth={} h={} seed={}\n",
                                  a.at.0, z, pick(i, a.seed, 14, 30), pick(i + 3, a.seed, 6, 14),
                                  pick(i + 6, a.seed, 4, 9), a.seed ^ i as u64),
                      estimate: 22 * 10 * 6 });
    }
    v.push(Part { what: "a sea cave".into(),
                  ir: format!("cave {},{} dir=0 len={} r=5 branches=2 seed={}\n",
                              a.at.0, a.at.1, r, a.seed ^ 0xc0a5),
                  estimate: r as i64 * 75 });
    v.push(Part { what: "the beach".into(),
                  ir: format!("layer barren {},{} {},{} density=0.15\n",
                              a.at.0 - r, a.at.1 - r, a.at.0, a.at.1 + r),
                  estimate: (r as i64).pow(2) / 10 });
    v
}

fn highlands(a: &FeatureArgs) -> Vec<Part> {
    let r = a.radius.clamp(60, 500);
    let mut v = Vec::new();
    v.push(Part { what: "the uplift".into(),
                  ir: format!("brush raise {},{} by={} r={} hill\n", a.at.0, a.at.1, (r / 5).clamp(10, 50), r),
                  estimate: (r as i64 * 2).pow(2) * 10 });
    v.push(Part { what: "roughening".into(),
                  ir: format!("brush roughen {},{} by=4 r={}\n", a.at.0, a.at.1, r),
                  estimate: (r as i64 * 2).pow(2) });
    // ravines cutting the plateau
    for i in 0..3 {
        let deg = pick(i, a.seed, 0, 359);
        let from = around(a.at, r, deg);
        let to = around(a.at, r / 4, deg + pick(i + 20, a.seed, 140, 220));
        v.push(Part { what: format!("ravine {}", i + 1),
                      ir: format!("canyon {},{} {},{} w={} depth={} seed={}\n",
                                  from.0, from.1, to.0, to.1, pick(i + 2, a.seed, 6, 14),
                                  pick(i + 9, a.seed, 12, 26), a.seed ^ i as u64),
                      estimate: r as i64 * 10 * 18 });
    }
    // tarns in the hollows
    for i in 0..2 {
        let p = around(a.at, r / 2, pick(i + 30, a.seed, 0, 359));
        v.push(Part { what: format!("tarn {}", i + 1),
                      ir: format!("lake {},{} r={} level={}\n", p.0, p.1, pick(i, a.seed, 8, 18),
                                  a.ground + (r / 6).clamp(4, 30)),
                      estimate: 300 });
    }
    v.push(Part { what: "the treeline".into(),
                  ir: format!("layer alpine {},{} {},{} density=0.4 above\n",
                              a.at.0 - r, a.at.1 - r, a.at.0 + r, a.at.1 + r),
                  estimate: (r as i64 * 2).pow(2) / 8 });
    v
}

fn sinkhole(a: &FeatureArgs) -> Vec<Part> {
    let r = a.radius.clamp(20, 200);
    let mut v = Vec::new();
    // a big cave first: the sinkhole is where its roof fell in
    v.push(Part { what: "the cavern below".into(),
                  ir: format!("cave {},{} dir={} len={} r={} branches=3 seed={}\n",
                              a.at.0, a.at.1, pick(0, a.seed, 0, 359), r * 2, (r / 5).clamp(4, 14), a.seed),
                  estimate: r as i64 * 2 * 100 });
    // the collapse: a shaft down to it
    v.push(Part { what: "the collapse".into(),
                  ir: format!("mesa {},{} r={} drop=0 seed={}\ncylinder {},{} r={} h={} block=air\n",
                              a.at.0, a.at.1, r, a.seed,
                              a.at.0, a.at.1, (r / 2).clamp(6, 60), (r / 2).clamp(8, 50)),
                  estimate: (r as i64 / 2).pow(2) * 3 * 20 });
    v.push(Part { what: "the pool at the bottom".into(),
                  ir: format!("lake {},{} r={} level={}\n", a.at.0, a.at.1, (r / 3).clamp(4, 30),
                              a.ground - (r / 2).clamp(8, 50) + 2),
                  estimate: 400 });
    v.push(Part { what: "a way down".into(),
                  ir: format!("stairs {},{} {},{} w=3\n",
                              a.at.0 + (r / 2).clamp(6, 60), a.at.1, a.at.0, a.at.1),
                  estimate: 200 });
    v.push(Part { what: "green at the bottom".into(),
                  ir: format!("layer jungle {},{} {},{} density=0.5\n",
                              a.at.0 - r / 2, a.at.1 - r / 2, a.at.0 + r / 2, a.at.1 + r / 2),
                  estimate: (r as i64).pow(2) / 8 });
    v
}

/// A feature suited to the site, when the player names none.
pub fn for_site(amb: &Ambient) -> &'static Feature {
    let s = amb.suggest();
    let name = match s.palette {
        "desert" | "mesa" => "badlands",
        "snowy" | "frozen" => "highlands",
        "volcanic" => "caldera",
        "jungle" => "sinkhole",
        _ => if amb.water > 0.2 { "cliffs" } else { "gorge" },
    };
    lookup(name).expect("every site feature is a real feature")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{parse_plan_at, Severity};

    fn args() -> FeatureArgs { FeatureArgs { at: (0, 0), radius: 140, seed: 7, ground: 72 } }

    #[test]
    fn every_feature_is_several_parts_of_valid_ir() {
        for f in FEATURES {
            let parts = (f.plan)(&args());
            assert!(parts.len() >= 4, "{} is a place, not one op: {} part(s)", f.name, parts.len());
            for p in &parts {
                assert!(!p.what.is_empty());
                let (plan, issues) = parse_plan_at(&p.ir, Some((0, 72, 0)));
                let errs: Vec<_> = issues.iter().filter(|i| i.severity == Severity::Error).collect();
                assert!(errs.is_empty(), "{}/{}: {:?}\n{}", f.name, p.what, errs, p.ir);
                assert!(!plan.ops.is_empty(), "{}/{} made no op", f.name, p.what);
                for l in p.ir.lines() {
                    if l.trim().is_empty() || l.trim_start().starts_with('"') || l.trim_start().starts_with('}') { continue; }
                    if l.trim_start().starts_with("scatter_json") { continue; }   // a block form
                    assert!(crate::plan::line_is_valid_ir(l), "{}/{}: {l:?}", f.name, p.what);
                }
            }
            // the estimate is a number the marker can show
            assert!(parts.iter().map(|p| p.estimate).sum::<i64>() > 1000, "{}", f.name);
        }
    }

    #[test]
    fn a_feature_is_deterministic_and_its_seed_changes_it() {
        let a = badlands(&args());
        let b = badlands(&args());
        assert_eq!(a, b);
        let c = badlands(&FeatureArgs { seed: 8, ..args() });
        assert_ne!(a, c, "a different seed lays the canyons differently");
        // same shape though: the same parts in the same order
        assert_eq!(a.iter().map(|p| &p.what).collect::<Vec<_>>(),
                   c.iter().map(|p| &p.what).collect::<Vec<_>>());
    }

    #[test]
    fn the_parts_are_ordered_so_each_cuts_into_the_last() {
        // badlands: the mesa is raised before the canyons cut it
        let p = badlands(&args());
        let mesa = p.iter().position(|p| p.what.contains("mesa")).unwrap();
        let canyon = p.iter().position(|p| p.what.contains("canyon")).unwrap();
        assert!(mesa < canyon, "the mesa stands before it is cut");
        let cover = p.iter().position(|p| p.what.contains("cover")).unwrap();
        assert!(canyon < cover, "the cover goes on after the cutting");
        // sinkhole: the cavern before the collapse into it
        let s = sinkhole(&args());
        assert!(s.iter().position(|p| p.what.contains("cavern")).unwrap()
                < s.iter().position(|p| p.what.contains("collapse")).unwrap());
        // caldera: rim up, floor down, then the lake in the hole
        let c = caldera(&args());
        let (rim, floor, lake) = (c.iter().position(|p| p.what.contains("rim")).unwrap(),
                                  c.iter().position(|p| p.what.contains("floor")).unwrap(),
                                  c.iter().position(|p| p.what.contains("lake")).unwrap());
        assert!(rim < floor && floor < lake);
    }

    #[test]
    fn the_radius_is_clamped_and_scales_the_parts() {
        let small = badlands(&FeatureArgs { radius: 1, ..args() });
        let big = badlands(&FeatureArgs { radius: 5000, ..args() });
        assert_eq!(small.len(), big.len());
        assert!(small.iter().map(|p| p.estimate).sum::<i64>() < big.iter().map(|p| p.estimate).sum::<i64>());
        // even clamped small, every part still parses
        for p in &small {
            let (_, issues) = parse_plan_at(&p.ir, Some((0, 72, 0)));
            assert!(issues.iter().all(|i| i.severity != Severity::Error), "{}: {issues:?}", p.what);
        }
    }

    #[test]
    fn the_site_picks_a_feature_that_belongs() {
        assert_eq!(for_site(&Ambient { surface_block: 12, ..Default::default() }).name, "badlands");
        assert_eq!(for_site(&Ambient { temperature: 0.1, ..Default::default() }).name, "highlands");
        assert_eq!(for_site(&Ambient { water: 0.5, ..Default::default() }).name, "cliffs");
        assert_eq!(for_site(&Ambient::default()).name, "gorge");
    }

    #[test]
    fn the_vocabulary_names_every_feature_for_the_prompt() {
        let v = vocabulary();
        for f in FEATURES { assert!(v.contains(f.name), "{} missing from the prompt", f.name); }
        assert!(v.is_ascii() && v.len() < 700, "{} chars", v.len());
    }
}
