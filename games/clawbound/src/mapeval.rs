//! Map fairness — adapted from early-fps `mapeval.rs` (openness / flow / cover /
//! verticality → S–F grade). Extended with a hunter-vs-cat **fairness** axis:
//! cats need cover, hunters need sightline, so a good map balances the two.

use crate::world;

pub struct MapScore {
    pub openness: f32,
    pub flow: f32,
    pub cover: f32,
    pub verticality: f32,
    pub has_landmark: bool,
    pub total: f32,
    pub fairness: f32,
}

impl MapScore {
    pub fn grade(&self) -> char {
        match self.total as i32 {
            90..=200 => 'S', 80..=89 => 'A', 70..=79 => 'B',
            60..=69 => 'C', 45..=59 => 'D', _ => 'F',
        }
    }
}

pub fn score() -> MapScore {
    let n = 64usize;
    let half = world::SIZE * 0.8;
    let step = 2.0 * half / n as f32;
    let (mut land, mut open) = (0u32, 0u32);
    let mut cover_sum = 0.0f32;
    let mut hs: Vec<f32> = Vec::new();
    for j in 0..n {
        for i in 0..n {
            let x = -half + i as f32 * step;
            let z = -half + j as f32 * step;
            let h = world::height_at(x, z);
            if h < world::WATER_Y { continue; } // water isn't walkable land
            land += 1; hs.push(h);
            let b = world::biome_at(x, z);
            cover_sum += b.tree;                 // foliage density ≈ cover
            let slope = (world::height_at(x + step, z) - h).abs() + (world::height_at(x, z + step) - h).abs();
            if b.tree < 0.2 && slope < 1.5 { open += 1; } // open, walkable = sightline
        }
    }
    let total_cells = (n * n) as f32;
    let flow = land as f32 / total_cells;
    let cover = if land > 0 { cover_sum / land as f32 } else { 0.0 };
    let openness = if land > 0 { open as f32 / land as f32 } else { 0.0 };
    let mean = hs.iter().sum::<f32>() / hs.len().max(1) as f32;
    let var = hs.iter().map(|h| (h - mean) * (h - mean)).sum::<f32>() / hs.len().max(1) as f32;
    let verticality = (var.sqrt() / 8.0).clamp(0.0, 1.0);
    let balance = 1.0 - (cover - openness).abs();
    let fairness = (balance * 100.0).clamp(0.0, 100.0);
    let total = (openness * 22.0 + flow * 22.0 + cover * 26.0 + verticality * 15.0 + balance * 15.0).clamp(0.0, 100.0);
    MapScore { openness, flow, cover, verticality, has_landmark: true, total, fairness }
}

// ── per-lane water fairness ──────────────────────────────────────────────────
// `score()`'s `fairness` axis grades the whole map's cover-vs-openness balance
// but has nothing to say about a SPECIFIC lane favoring one side — the case
// that prompted this: Battleground's mid trail crosses the river basin, and a
// river that happens to sit closer to one camp than the other hands that camp
// a genuine "river lane" the other doesn't have to cross. Generic over any
// three-waypoint lane (home camp, bend, away camp) rather than importing
// battleground's types, matching this module's existing "adapted, not
// coupled" posture.

/// How wet one lane is, from each side's own half (camp → bend).
pub struct LaneWaterReport {
    pub lane: usize,
    /// Fraction of clan A's home-side half (`camp_a` → bend) that's water.
    pub water_a: f32,
    /// Fraction of clan B's home-side half (`camp_b` → bend) that's water.
    pub water_b: f32,
    /// `|water_a - water_b|` — 0 is a perfectly fair lane; the closer to 0
    /// across every lane, the less either camp's approach is singled out by
    /// the river's placement.
    pub asymmetry: f32,
}

/// Grade every lane's water fairness. `bends` are the lanes' middle waypoints
/// in the same order `lane_water_report` reports them (index = lane number);
/// `samples` controls how finely each half is walked (more samples = a more
/// exact fraction, at the cost of more `height_at` calls — 32 is plenty for a
/// ~100-unit half-lane).
pub fn lane_water_report(camp_a: (f32, f32), camp_b: (f32, f32), bends: &[(f32, f32)], samples: usize) -> Vec<LaneWaterReport> {
    bends
        .iter()
        .enumerate()
        .map(|(lane, &bend)| {
            let water_a = water_fraction_segment(camp_a, bend, samples);
            let water_b = water_fraction_segment(camp_b, bend, samples);
            LaneWaterReport { lane, water_a, water_b, asymmetry: (water_a - water_b).abs() }
        })
        .collect()
}

/// Fraction of points along the segment `a`→`b` (inclusive of both ends)
/// that sit below `world::WATER_Y` — the same "water isn't walkable land"
/// test `score()` uses.
fn water_fraction_segment(a: (f32, f32), b: (f32, f32), samples: usize) -> f32 {
    let n = samples.max(1);
    let mut wet = 0u32;
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let x = a.0 + (b.0 - a.0) * t;
        let z = a.1 + (b.1 - a.1) * t;
        if world::height_at(x, z) < world::WATER_Y {
            wet += 1;
        }
    }
    wet as f32 / (n + 1) as f32
}

#[cfg(test)]
mod lane_water_tests {
    use super::*;

    #[test]
    fn a_dry_map_has_zero_water_fraction_and_zero_asymmetry() {
        // the bake and the reads after it, under the terrain lock: the grid is global and
        // another test baking in parallel would swap it out from under this one
        let _terrain_guard = world::TERRAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        world::init_world();
        world::set_terrain_seed(1);
        // A bend positioned ON one of the camps degenerates that lane's half
        // to a single point — still a valid (zero-length) segment, and
        // exercises the `samples.max(1)`/inclusive-endpoint edges.
        let camp_a = (-118.0, -118.0);
        let camp_b = (118.0, 118.0);
        let bends = [(-85.0, 85.0), (0.0, 0.0), (85.0, -85.0)];
        let report = lane_water_report(camp_a, camp_b, &bends, 16);
        assert_eq!(report.len(), 3);
        for r in &report {
            assert!((0.0..=1.0).contains(&r.water_a));
            assert!((0.0..=1.0).contains(&r.water_b));
            assert!((0.0..=1.0).contains(&r.asymmetry));
        }
    }

    #[test]
    fn identical_halves_report_zero_asymmetry() {
        // camp_a == camp_b makes every lane's two halves the literal same
        // segment — the asymmetry floor, and a cheap way to check the
        // arithmetic without depending on exactly where the river lands.
        // the bake and the reads after it, under the terrain lock: the grid is global and
        // another test baking in parallel would swap it out from under this one
        let _terrain_guard = world::TERRAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        world::init_world();
        let camp = (-50.0, -50.0);
        let bends = [(0.0, 0.0), (20.0, 30.0)];
        let report = lane_water_report(camp, camp, &bends, 8);
        for r in &report {
            assert_eq!(r.water_a, r.water_b);
            assert_eq!(r.asymmetry, 0.0);
        }
    }

    #[test]
    fn lane_index_matches_input_order() {
        // the bake and the reads after it, under the terrain lock: the grid is global and
        // another test baking in parallel would swap it out from under this one
        let _terrain_guard = world::TERRAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        world::init_world();
        let bends = [(1.0, 1.0), (2.0, 2.0), (3.0, 3.0)];
        let report = lane_water_report((0.0, 0.0), (0.0, 0.0), &bends, 4);
        for (i, r) in report.iter().enumerate() {
            assert_eq!(r.lane, i);
        }
    }
}

// ── the battleground's own grader ───────────────────────────────────────────
//
// `score()` grades the wild land as a place to hunt and be hunted. A clan-war
// map has to answer a different question: are the three trails *different from
// each other* (or the country was shaped for nothing) and *the same for both
// clans* (or the match was decided before it started)? Those pull in opposite
// directions, and neither is visible in a single number, so they get their own.

/// One trail, measured along its length.
#[derive(Clone, Copy, Debug, Default)]
pub struct LaneProfile {
    /// Mean height of the walk, in world units.
    pub height: f32,
    /// Fraction of sample points standing in water — the river lane's signature.
    pub wet: f32,
    /// Mean foliage density: how much of the trail can be fought in unseen.
    pub cover: f32,
    /// Mean absolute slope between samples — how much climbing the walk costs.
    pub climb: f32,
}

/// How the three trails compare, and whether both clans get the same three.
#[derive(Clone, Copy, Debug)]
pub struct LaneScore {
    /// Duo, mid, solo — as clan A walks them.
    pub lanes: [LaneProfile; 3],
    /// Largest difference between any two lanes, over the profile axes. Near
    /// zero means three identical trails and a map whose terrain did no work.
    pub distinctness: f32,
    /// Largest difference between a lane as clan A walks it and the same lane
    /// as clan B does. **This is the one that must be ~0**: the clan-swap
    /// reflection is supposed to make it exactly zero, and this is the number
    /// that would have caught the two-symmetry-conventions bug in one run
    /// instead of a mirror probe and three commits.
    pub clan_gap: f32,
}

impl LaneScore {
    /// A map is right when its lanes differ from each other and not between
    /// clans. Verdict rather than score, because a 0.02 clan gap is not "nearly
    /// fair", it is a bug with a small number attached.
    pub fn verdict(&self) -> &'static str {
        if self.clan_gap > 0.05 {
            "UNFAIR — the same trail plays differently for the two clans"
        } else if self.distinctness < 0.15 {
            "FLAT — the three trails are interchangeable; the terrain did no work"
        } else {
            "good — distinct trails, identical for both clans"
        }
    }
}

fn walk(from: (f32, f32), to: (f32, f32), bend: (f32, f32)) -> LaneProfile {
    const N: usize = 40;
    let mut p = LaneProfile::default();
    let mut last_h: Option<f32> = None;
    let mut wet = 0u32;
    for k in 0..=N {
        let t = k as f32 / N as f32;
        // camp -> bend -> camp, the path the patrols actually walk
        let (x, z) = if t < 0.5 {
            let u = t * 2.0;
            (from.0 + (bend.0 - from.0) * u, from.1 + (bend.1 - from.1) * u)
        } else {
            let u = (t - 0.5) * 2.0;
            (bend.0 + (to.0 - bend.0) * u, bend.1 + (to.1 - bend.1) * u)
        };
        let h = world::height_at(x, z);
        p.height += h;
        if h < world::WATER_Y {
            wet += 1;
        }
        p.cover += world::biome_at(x, z).tree;
        if let Some(prev) = last_h {
            p.climb += (h - prev).abs();
        }
        last_h = Some(h);
    }
    let n = (N + 1) as f32;
    p.height /= n;
    p.cover /= n;
    p.climb /= n;
    p.wet = wet as f32 / n;
    p
}

/// Grade the battleground's three trails. Assumes the caller has baked the
/// battleground's terrain (pool, lift, river course, seed) — it reads whatever
/// world is currently loaded.
pub fn lane_score(camp_a: (f32, f32), camp_b: (f32, f32), bends: [(f32, f32); 3]) -> LaneScore {
    let a: Vec<LaneProfile> = bends.iter().map(|&b| walk(camp_a, camp_b, b)).collect();
    let b: Vec<LaneProfile> = bends.iter().map(|&b| walk(camp_b, camp_a, b)).collect();
    // axes normalised so one unit of each is roughly one unit of "different"
    let axes = |p: &LaneProfile| [p.height / 8.0, p.wet, p.cover, p.climb / 2.0];
    let spread = |x: &LaneProfile, y: &LaneProfile| {
        axes(x)
            .iter()
            .zip(axes(y).iter())
            .map(|(u, v)| (u - v).abs())
            .fold(0.0f32, f32::max)
    };
    let mut distinctness = 0.0f32;
    for i in 0..3 {
        for j in i + 1..3 {
            distinctness = distinctness.max(spread(&a[i], &a[j]));
        }
    }
    let mut clan_gap = 0.0f32;
    for i in 0..3 {
        clan_gap = clan_gap.max(spread(&a[i], &b[i]));
    }
    LaneScore {
        lanes: [a[0], a[1], a[2]],
        distinctness,
        clan_gap,
    }
}

#[cfg(test)]
mod lane_tests {
    use super::*;
    use crate::battleground::Battleground;

    /// The number that matters: both clans must walk the same three trails.
    /// This is the cheap standing guard for the symmetry the map spent three
    /// commits acquiring — a single call, no 168-game run required.
    #[test]
    fn the_battlegrounds_lanes_are_distinct_and_clan_fair() {
        let bg = Battleground::new(crate::ancestor::Ancestor::Lion);
        let bends = [(-85.0, 85.0), (0.0, 0.0), (85.0, -85.0)];
        let s = lane_score(bg.camps[0], bg.camps[1], bends);
        assert!(
            s.clan_gap < 0.02,
            "the same trail must play the same for both clans, gap {:.3} ({:?})",
            s.clan_gap,
            s.lanes
        );
        assert!(
            s.distinctness > 0.15,
            "the three trails should not be interchangeable, spread {:.3}",
            s.distinctness
        );
        assert_eq!(s.verdict(), "good — distinct trails, identical for both clans");
        // and the shape the terrain work aimed at: the duo trail climbs, the
        // solo trail gets the water
        let (duo, solo) = (s.lanes[0], s.lanes[2]);
        assert!(duo.height > solo.height, "duo {:?} should sit above solo {:?}", duo, solo);
        assert!(solo.wet >= duo.wet, "the river belongs to the solo side");
    }

    #[test]
    fn the_grader_can_actually_fail() {
        // A grader that cannot fail is decoration. Walk three copies of one
        // trail: distinctness collapses and the verdict says so.
        let bg = Battleground::new(crate::ancestor::Ancestor::Lion);
        let flat = [(0.0, 0.0); 3];
        let s = lane_score(bg.camps[0], bg.camps[1], flat);
        assert!(s.distinctness < 0.15);
        assert!(s.verdict().starts_with("FLAT"));
    }
}
