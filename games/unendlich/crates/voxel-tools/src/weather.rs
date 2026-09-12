//! Weathering: turning a build into a ruin.
//!
//! The corpus says "abandoned" 10 times, "ruin" 7, plus "overgrown",
//! "crumbling", "broken". It is one of the most common things a builder
//! does to a finished structure — and it is not a SHAPE, so it does not
//! belong in the dictionary. It is a transform over blocks that already
//! exist.
//!
//! `restyle` swaps a palette wholesale, which produces a clean building
//! made of mossy stone: a different building, not a damaged one. Decay
//! has to be uneven, and where it is uneven is the whole craft.
//!
//! THREE RULES, ALL FROM WHAT BUILDERS ACTUALLY DESCRIBE
//!
//! 1. **Damage is edge-biased.** A wall crumbles at its corners and its
//!    top, not in the middle of an intact face. Uniform random removal
//!    reads as moth-eaten; edge-weighted removal reads as ruined.
//! 2. **Overgrowth is gravity-aware.** Moss climbs from the ground and
//!    pools on upward-facing surfaces; vines hang from overhangs. Moss
//!    sprayed uniformly over a wall looks like paint.
//! 3. **Rubble lands below.** A block removed from a wall should appear
//!    at the foot of it. This is what makes a ruin look like it fell
//!    rather than being built with gaps.
//!
//! Deterministic from a seed, so a preview and the commit agree — the
//! same reason `scatter` is seeded.

use crate::VoxelWorld;

/// How far gone. All fields 0.0..=1.0.
#[derive(Debug, Clone, Copy)]
pub struct Weather {
    /// Fraction of edge-exposed blocks removed.
    pub damage: f32,
    /// Fraction of eligible surfaces that take moss.
    pub moss: f32,
    /// Fraction of overhangs that grow a vine.
    pub vines: f32,
    /// Whether removed blocks reappear as rubble at the base.
    pub rubble: bool,
    pub seed: u64,
}

impl Default for Weather {
    fn default() -> Self {
        Self { damage: 0.25, moss: 0.35, vines: 0.2, rubble: true, seed: 1 }
    }
}

impl Weather {
    /// Presets, because "abandoned" and "ancient" are what a player
    /// says and 0.25/0.35/0.2 is not.
    pub fn preset(name: &str) -> Self {
        match name {
            "worn" => Self { damage: 0.08, moss: 0.2, vines: 0.05, ..Default::default() },
            "ancient" => Self { damage: 0.45, moss: 0.6, vines: 0.45, ..Default::default() },
            "burnt" => Self { damage: 0.4, moss: 0.0, vines: 0.0, ..Default::default() },
            // "abandoned" is the corpus's own word and the sensible middle
            _ => Self::default(),
        }
    }
}

/// One change the weathering wants made.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Change {
    pub at: (i32, i32, i32),
    pub to: u16,
}

fn hash3(x: i32, y: i32, z: i32, seed: u64) -> u64 {
    let mut h = seed
        ^ (x as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (y as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ (z as i64 as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= h >> 31;
    h = h.wrapping_mul(0x7FB5_D329_728E_A185);
    h ^ (h >> 27)
}

/// 0.0..1.0 from a cell and a salt.
fn roll(x: i32, y: i32, z: i32, seed: u64, salt: u64) -> f32 {
    (hash3(x, y, z, seed ^ salt) % 10_000) as f32 / 10_000.0
}

/// How exposed a cell is: the fraction of its six neighbours that are
/// air. This is the edge-bias in one number — a block in the middle of
/// a face scores low, a corner scores high, and damage follows it.
pub fn exposure<W: VoxelWorld>(w: &W, x: i32, y: i32, z: i32) -> f32 {
    let n = [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)]
        .iter()
        .filter(|(dx, dy, dz)| w.get(x + dx, y + dy, z + dz) == 0)
        .count();
    n as f32 / 6.0
}

/// Plan the weathering of a region. Pure: reads the world, returns
/// changes, writes nothing. The caller previews or applies them.
pub fn plan<W: VoxelWorld>(w: &W, min: (i32, i32, i32), max: (i32, i32, i32),
                           cfg: &Weather, moss_block: u16, vine_block: u16,
                           rubble_block: u16) -> Vec<Change> {
    use std::collections::BTreeSet;
    let (lo, hi) = (
        (min.0.min(max.0), min.1.min(max.1), min.2.min(max.2)),
        (min.0.max(max.0), min.1.max(max.1), min.2.max(max.2)));

    // TWO PHASES, and the order is the whole point.
    //
    // Moss grows on upward-facing surfaces. In an INTACT wall the only
    // upward face is the top course, so computing moss against the
    // original world puts a green stripe along the parapet and nothing
    // else. In a ruin the broken tops are where moss actually grows —
    // which means damage has to happen first and moss has to see it.
    // The first version of this did them in one pass and produced
    // exactly the stripe.
    // BTreeSet, not HashSet: the rubble pass iterates this, and
    // HashSet order varies run to run, so the same seed produced the
    // same SET of changes in a different ORDER. The determinism test
    // caught it — a preview and a commit disagreeing on order is a
    // preview you cannot trust.
    let mut gone: BTreeSet<(i32, i32, i32)> = BTreeSet::new();
    let mut out = Vec::new();

    for x in lo.0..=hi.0 {
        for y in lo.1..=hi.1 {
            for z in lo.2..=hi.2 {
                if w.get(x, y, z) == 0 { continue; }
                let e = exposure(w, x, y, z);
                if e == 0.0 { continue; }   // buried: nothing reaches it
                // Weighted by exposure: a corner goes long before the
                // middle of a face, which is the difference between
                // ruined and moth-eaten.
                if roll(x, y, z, cfg.seed, 1) < cfg.damage * e {
                    out.push(Change { at: (x, y, z), to: 0 });
                    gone.insert((x, y, z));
                }
            }
        }
    }

    // Occupancy AFTER the damage pass.
    let solid = |x: i32, y: i32, z: i32| {
        !gone.contains(&(x, y, z)) && w.get(x, y, z) != 0
    };

    for x in lo.0..=hi.0 {
        for y in lo.1..=hi.1 {
            for z in lo.2..=hi.2 {
                if !solid(x, y, z) { continue; }
                // MOSS on newly-exposed upward faces, thicker low down:
                // it climbs from the ground rather than raining.
                if !solid(x, y + 1, z) {
                    let h = (hi.1 - lo.1).max(1) as f32;
                    let climb = 1.0 - ((y - lo.1) as f32 / h).min(1.0);
                    if roll(x, y, z, cfg.seed, 2) < cfg.moss * (0.25 + 0.75 * climb) {
                        out.push(Change { at: (x, y, z), to: moss_block });
                        continue;
                    }
                }
                // VINES hang beneath overhangs.
                if !solid(x, y - 1, z) && roll(x, y, z, cfg.seed, 3) < cfg.vines {
                    out.push(Change { at: (x, y - 1, z), to: vine_block });
                }
            }
        }
    }

    // RUBBLE. What fell has to land somewhere, or the ruin reads as a
    // building constructed with holes in it.
    //
    // It cannot land straight down: a block removed from mid-wall has
    // wall beneath it, so a vertical drop finds nothing and no rubble
    // ever appears. Debris falls OUTWARD, to whichever neighbouring
    // column is open — which is where it lies against a real ruin.
    if cfg.rubble {
        for &(x, y, z) in &gone {
            let r = roll(x, y, z, cfg.seed, 4);
            if r >= 0.6 { continue; }
            let dir = [(1, 0), (-1, 0), (0, 1), (0, -1)]
                [(hash3(x, y, z, cfg.seed ^ 5) % 4) as usize];
            let (ox, oz) = (x + dir.0, z + dir.1);
            let mut fy = y;
            let floor = lo.1 - 6;
            while fy > floor && !solid(ox, fy, oz) { fy -= 1; }
            let land = fy + 1;
            if land <= y && !solid(ox, land, oz) {
                out.push(Change { at: (ox, land, oz), to: rubble_block });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct W { cells: BTreeMap<(i32, i32, i32), u16> }
    impl W {
        /// A hollow stone box: walls with an intact interior face.
        fn tower() -> Self {
            let mut cells = BTreeMap::new();
            for x in 0..9 { for y in 0..12 { for z in 0..9 {
                if x == 0 || x == 8 || z == 0 || z == 8 {
                    cells.insert((x, y, z), 1u16);
                }
            }}}
            W { cells }
        }
    }
    impl VoxelWorld for W {
        fn get(&self, x: i32, y: i32, z: i32) -> u16 {
            if let Some(&b) = self.cells.get(&(x, y, z)) { return b; }
            if y < 0 { 1 } else { 0 }
        }
        fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool {
            self.cells.insert((x, y, z), b); true
        }
    }

    #[test]
    fn damage_prefers_edges_over_the_middle_of_a_face() {
        // The rule that separates "ruined" from "moth-eaten". A corner
        // column has more air around it than a face-centre column, so it
        // must lose more blocks.
        let w = W::tower();
        let cfg = Weather { damage: 0.9, moss: 0.0, vines: 0.0, rubble: false, seed: 3 };
        let changes = plan(&w, (0, 0, 0), (8, 11, 8), &cfg, 23, 29, 4);
        let removed = |x: i32, z: i32| changes.iter()
            .filter(|c| c.to == 0 && c.at.0 == x && c.at.2 == z).count();
        let corner = removed(0, 0);
        let face = removed(4, 0);
        assert!(corner > face,
                "a corner ({corner}) must erode faster than a face centre ({face})");
    }

    #[test]
    fn a_buried_block_never_weathers() {
        // exposure 0 means nothing reaches it. A solid block of stone
        // should come out of a weathering pass untouched inside.
        let mut cells = BTreeMap::new();
        for x in 0..5 { for y in 0..5 { for z in 0..5 { cells.insert((x, y, z), 1u16); }}}
        let w = W { cells };
        let cfg = Weather { damage: 1.0, moss: 1.0, vines: 1.0, rubble: false, seed: 1 };
        let changes = plan(&w, (0, 0, 0), (4, 4, 4), &cfg, 23, 29, 4);
        assert!(!changes.iter().any(|c| c.at == (2, 2, 2)), "the core is untouched");
        assert!(!changes.is_empty(), "but the surface is not");
    }

    #[test]
    fn moss_favours_the_bottom_and_upward_faces() {
        let w = W::tower();
        // With NO damage an intact wall has one upward face (the top
        // course), so moss can only appear there. Damage first, and the
        // broken tops are what moss actually grows on.
        let cfg = Weather { damage: 0.35, moss: 0.9, vines: 0.0, rubble: false, seed: 5 };
        let changes = plan(&w, (0, 0, 0), (8, 11, 8), &cfg, 23, 29, 4);
        let moss: Vec<i32> = changes.iter().filter(|c| c.to == 23).map(|c| c.at.1).collect();
        assert!(!moss.is_empty(), "some moss");
        // Measure the RATE, not the count. The tower's top course is
        // structurally guaranteed to have an open upward face, so it
        // offers far more sites than the damaged tops lower down and
        // swamps a raw comparison. What the rule actually claims is that
        // a given exposed face low down is MORE LIKELY to be mossed.
        let gone: std::collections::HashSet<(i32,i32,i32)> =
            changes.iter().filter(|c| c.to == 0).map(|c| c.at).collect();
        let sites = |band: std::ops::RangeInclusive<i32>| {
            let mut n = 0;
            for x in 0..9 { for z in 0..9 { for y in band.clone() {
                let here = w.get(x, y, z) != 0 && !gone.contains(&(x, y, z));
                let above_open = w.get(x, y + 1, z) == 0 || gone.contains(&(x, y + 1, z));
                if here && above_open { n += 1; }
            }}}
            n
        };
        let rate = |band: std::ops::RangeInclusive<i32>, n: usize| {
            let s = sites(band);
            if s == 0 { 0.0 } else { n as f32 / s as f32 }
        };
        let low = moss.iter().filter(|y| **y <= 3).count();
        let high = moss.iter().filter(|y| **y >= 8).count();
        let (lr, hr) = (rate(0..=3, low), rate(8..=11, high));
        assert!(lr > hr, "moss favours low exposed faces: rate {lr:.2} low vs {hr:.2} high");
        // and only where the top face is open -- the tower's top course
        // every mossed cell has an open top AFTER damage
        let gone: std::collections::HashSet<(i32,i32,i32)> =
            changes.iter().filter(|c| c.to == 0).map(|c| c.at).collect();
        for c in changes.iter().filter(|c| c.to == 23) {
            let above = (c.at.0, c.at.1 + 1, c.at.2);
            assert!(w.get(above.0, above.1, above.2) == 0 || gone.contains(&above),
                    "moss needs an open top face at {above:?}");
        }
    }

    #[test]
    fn what_falls_lands_at_the_foot_of_the_wall() {
        let w = W::tower();
        let cfg = Weather { damage: 0.6, moss: 0.0, vines: 0.0, rubble: true, seed: 11 };
        let changes = plan(&w, (0, 0, 0), (8, 11, 8), &cfg, 23, 29, 4);
        let rubble: Vec<&Change> = changes.iter().filter(|c| c.to == 4).collect();
        assert!(!rubble.is_empty(), "a ruin needs rubble, or it reads as built with holes");
        // rubble sits low, not where the block was
        let avg_y: f32 = rubble.iter().map(|c| c.at.1 as f32).sum::<f32>() / rubble.len() as f32;
        assert!(avg_y < 6.0, "rubble collects near the base, mean y = {avg_y}");
    }

    #[test]
    fn the_same_seed_gives_the_same_ruin() {
        // A preview that disagrees with the commit is worse than no
        // preview; this is why scatter is seeded and so is this.
        let w = W::tower();
        let cfg = Weather::preset("abandoned");
        let a = plan(&w, (0, 0, 0), (8, 11, 8), &cfg, 23, 29, 4);
        let b = plan(&w, (0, 0, 0), (8, 11, 8), &cfg, 23, 29, 4);
        assert_eq!(a, b);
        let other = Weather { seed: 99, ..cfg };
        assert_ne!(a, plan(&w, (0, 0, 0), (8, 11, 8), &other, 23, 29, 4));
    }

    #[test]
    fn presets_are_ordered_the_way_the_words_are() {
        let w = W::tower();
        let count = |name: &str| {
            plan(&w, (0, 0, 0), (8, 11, 8), &Weather::preset(name), 23, 29, 4)
                .iter().filter(|c| c.to == 0).count()
        };
        let (worn, abandoned, ancient) = (count("worn"), count("abandoned"), count("ancient"));
        assert!(worn < abandoned && abandoned < ancient,
                "worn {worn} < abandoned {abandoned} < ancient {ancient}");
    }
}
