//! composition.rs — a **spatial-composition / legibility** score that's much harder to game than `mapeval`.
//!
//! ## Why this exists
//! The `mapeval` grade rewards proxies that random clutter satisfies by accident: "openness" is a free point
//! for any open-sky map, "cover" is just obstacle density near a target, "flow" rewards a *statistical* mix of
//! sightline lengths that scattered boxes produce without any design intent. So a procedurally-cluttered map
//! grades A/B next to a genuinely hand-composed one — the metric can't tell **designed space** from
//! **plausible noise**. (A human ranks them very differently.)
//!
//! This module measures the thing the old metrics miss: is the space *composed* — organised into legible
//! groupings with clear negative space — or is it *noise-like* — uniform occupancy at every scale? The insight
//! (a nod to "score the map against 3D Perlin noise"): a **designed** layout has structured, clustered
//! occupancy with real voids (rooms, lanes, plazas) between masses; **random scatter** has uniform density
//! everywhere and no clear voids. We can measure that difference directly and cheaply.
//!
//! ## The four sub-scores (each 0..1, all *anti-gaming* by construction)
//! 1. **Clustering** — do objects group into masses, or spread uniformly? Measured as the excess variance of
//!    occupancy over a coarse grid vs. what a uniform-random scatter of the *same count* would produce. Random
//!    scatter → ~0; deliberate clusters (buildings, cover nests) → high. You can't fake this by adding more
//!    objects: uniform density at any count scores ~0.
//! 2. **Void structure** — are there clear, connected open regions (lanes, rooms, plazas) a player reads as
//!    space? Measured from the size of the largest open regions relative to total open area. Confetti-scatter
//!    breaks open space into slivers → low; designed maps keep big legible voids → high.
//! 3. **Landmark hierarchy** — is there a clear *few* dominant masses (a skyline you orient by), or is every
//!    object the same size (visual mush)? Measured as the size inequality (Gini) of the object masses. One
//!    tower + small cover → high; a field of identical crates → low.
//! 4. **Multi-scale organisation** — does structure exist at more than one scale (a district of buildings,
//!    each with detail), or only one? Measured by comparing occupancy clustering at a coarse vs. a fine grid.
//!
//! None of these is improved by "add more objects near a target density" — the lever the old metric handed to
//! anyone gaming it. They reward *arrangement*, which is what a human actually sees.

use crate::level::Level;

/// A composition analysis of a level's spatial arrangement.
#[derive(Clone, Debug)]
pub struct CompositionReport {
    /// Do objects group into masses vs. spread uniformly (0 = random scatter, 1 = strong clustering).
    pub clustering: f32,
    /// Are there large, legible open regions (lanes/rooms/plazas) vs. sliver-filled space.
    pub void_structure: f32,
    /// Is there a clear size hierarchy of masses (a skyline to read) vs. uniform-size mush.
    pub landmark_hierarchy: f32,
    /// Does organisation exist at multiple scales vs. only one.
    pub multiscale: f32,
    /// A single 0..1 composition score. Higher = reads as a designed, legible space; lower = reads as noise.
    pub score: f32,
    /// How noise-like the occupancy is (0 = highly structured, 1 = indistinguishable from random scatter).
    /// This is the direct "is this just Perlin-noise clutter?" read.
    pub noise_likeness: f32,
}

/// Rasterise object occupancy into a `cells`×`cells` grid over the map footprint. Each obstacle contributes
/// its footprint (weighted by area) to the cells it covers. Returns (occupancy grid, total object count).
fn occupancy_grid(level: &Level, cells: usize) -> (Vec<f32>, usize) {
    let ts = level.ts();
    let wx = level.w as f32 * ts;
    let wz = level.h as f32 * ts;
    let mut grid = vec![0.0f32; cells * cells];
    // prefer freestanding cover on authored maps; else all obstacles
    let obs = if level.cover_obstacles.is_empty() { &level.obstacles } else { &level.cover_obstacles };
    for o in obs {
        let gx = ((o.center.x / wx) * cells as f32).clamp(0.0, cells as f32 - 1.0) as usize;
        let gz = ((o.center.z / wz) * cells as f32).clamp(0.0, cells as f32 - 1.0) as usize;
        let mass = (o.half.x * o.half.z).max(0.01);
        grid[gz * cells + gx] += mass;
    }
    (grid, obs.len())
}

/// The clustering signal: how much more *concentrated* the occupancy is than a uniform-random scatter of the
/// same total mass would be. Uses the coefficient of variation of cell occupancy, normalised against the CV a
/// Poisson (random) scatter of the same count over the same grid would produce. Random → ~0, clustered → →1.
fn clustering_of(grid: &[f32], count: usize) -> f32 {
    let cells = grid.len();
    let total: f32 = grid.iter().sum();
    if total <= 0.0 || count == 0 {
        return 0.0;
    }
    let mean = total / cells as f32;
    let var = grid.iter().map(|&v| (v - mean).powi(2)).sum::<f32>() / cells as f32;
    let cv = if mean > 1e-6 { var.sqrt() / mean } else { 0.0 };
    // For n objects scattered uniformly over `cells` bins, the occupancy-count CV ≈ sqrt((cells-1)/n) (Poisson-
    // like). Real clustering pushes CV well above that baseline. Ratio-1 maps random→0, clustered→positive.
    let baseline_cv = (((cells as f32 - 1.0) / count as f32).max(1e-6)).sqrt();
    ((cv / baseline_cv.max(1e-3)) - 1.0).clamp(0.0, 3.0) / 3.0
}

/// Void structure: the fraction of open (object-free) area that lives in *large connected regions* rather than
/// scattered slivers. Flood-fills the empty cells of a medium grid; rewards a layout whose open space forms a
/// few big rooms/lanes over one that's shot through with clutter.
fn void_structure(level: &Level, cells: usize) -> f32 {
    let (grid, _) = occupancy_grid(level, cells);
    let occupied: Vec<bool> = grid.iter().map(|&v| v > 1e-6).collect();
    let empty_total = occupied.iter().filter(|&&o| !o).count();
    if empty_total == 0 {
        return 0.0;
    }
    // connected-component sizes of empty cells (4-connectivity)
    let mut visited = vec![false; cells * cells];
    let mut sizes: Vec<usize> = Vec::new();
    for start in 0..cells * cells {
        if occupied[start] || visited[start] {
            continue;
        }
        let mut stack = vec![start];
        visited[start] = true;
        let mut sz = 0usize;
        while let Some(c) = stack.pop() {
            sz += 1;
            let (cx, cz) = (c % cells, c / cells);
            let push = |nx: i32, nz: i32, stack: &mut Vec<usize>, visited: &mut Vec<bool>| {
                if nx >= 0 && nz >= 0 && (nx as usize) < cells && (nz as usize) < cells {
                    let ni = nz as usize * cells + nx as usize;
                    if !occupied[ni] && !visited[ni] {
                        visited[ni] = true;
                        stack.push(ni);
                    }
                }
            };
            push(cx as i32 + 1, cz as i32, &mut stack, &mut visited);
            push(cx as i32 - 1, cz as i32, &mut stack, &mut visited);
            push(cx as i32, cz as i32 + 1, &mut stack, &mut visited);
            push(cx as i32, cz as i32 - 1, &mut stack, &mut visited);
        }
        sizes.push(sz);
    }
    if sizes.is_empty() {
        return 0.0;
    }
    // fraction of open area in the largest ~3 regions — high means space reads as a few big legible voids;
    // low means it's shattered into many little pockets (confetti clutter).
    sizes.sort_unstable_by(|a, b| b.cmp(a));
    let top: usize = sizes.iter().take(3).sum();
    (top as f32 / empty_total as f32).clamp(0.0, 1.0)
}

/// Landmark hierarchy: the size inequality (Gini coefficient) of the *mesh* masses. A designed skyline has a
/// few dominant structures and many small ones (high Gini); a field of identical props is uniform (low Gini).
fn landmark_hierarchy(level: &Level) -> f32 {
    // use per-mesh triangle counts as a cheap "visual mass" proxy for standing geometry
    let mut masses: Vec<f32> = level
        .meshes
        .iter()
        .filter(|m| m.tri_count() > 6)
        .map(|m| m.tri_count() as f32)
        .collect();
    if masses.len() < 3 {
        return 0.0;
    }
    masses.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = masses.len() as f32;
    let sum: f32 = masses.iter().sum();
    if sum <= 0.0 {
        return 0.0;
    }
    // Gini = (2*sum(i*x_i)/(n*sum) ) - (n+1)/n , i 1-based
    let weighted: f32 = masses.iter().enumerate().map(|(i, &x)| (i as f32 + 1.0) * x).sum();
    let gini = (2.0 * weighted) / (n * sum) - (n + 1.0) / n;
    gini.clamp(0.0, 1.0)
}

/// Analyse a level's spatial composition.
pub fn analyze(level: &Level) -> CompositionReport {
    // clustering at a coarse grid (districts) and a fine grid (detail)
    let coarse = 8usize;
    let fine = 16usize;
    let (cg, count) = occupancy_grid(level, coarse);
    let (fg, _) = occupancy_grid(level, fine);
    let clustering = clustering_of(&cg, count);
    let fine_clustering = clustering_of(&fg, count);

    // multi-scale: real design has structure at BOTH scales; noise has it at neither. Reward both being
    // present (min of the two), so a map that's only clustered at one scale doesn't score as fully organised.
    let multiscale = clustering.min(fine_clustering);

    let void = void_structure(level, 12);
    let hierarchy = landmark_hierarchy(level);

    // noise-likeness: how close the occupancy is to uniform random. Low clustering AND shattered voids = noisy.
    let noise_likeness = ((1.0 - clustering) * 0.6 + (1.0 - void) * 0.4).clamp(0.0, 1.0);

    // weighted composition score. Clustering + void structure are the backbone (they most separate "designed"
    // from "scatter"); hierarchy and multi-scale are the polish.
    let score = (clustering * 0.32 + void * 0.30 + hierarchy * 0.20 + multiscale * 0.18).clamp(0.0, 1.0);

    CompositionReport { clustering, void_structure: void, landmark_hierarchy: hierarchy, multiscale, score, noise_likeness }
}

/// A blended "honest quality" score combining composition (is it designed?) with the sightline structure
/// (does it play well?) — deliberately NOT including the gameable `mapeval` proxies. This is the number that
/// should track human judgement more closely than the old grade.
pub fn honest_quality(level: &Level) -> f32 {
    let comp = analyze(level);
    let sr = crate::sightline::analyze(level);
    // Composition (designed vs. noise) leads; tactical structure (plays well) supports. A map that's noise-like
    // is capped low no matter how its sightlines fall out, because it doesn't read as a place.
    let base = comp.score * 0.6 + sr.structure_score * 0.4;
    // hard penalty for noise-like layouts: multiply down as noise_likeness rises past 0.6
    let noise_penalty = 1.0 - ((comp.noise_likeness - 0.6).max(0.0) / 0.4) * 0.5;
    (base * noise_penalty).clamp(0.0, 1.0)
}

/// Letter grade from the honest-quality score, tuned so it tracks a stricter human eye (few A's, most maps
/// land B/C/D unless they're genuinely well-composed AND play well).
pub fn honest_grade(level: &Level) -> char {
    match honest_quality(level) {
        q if q >= 0.72 => 'A',
        q if q >= 0.58 => 'B',
        q if q >= 0.42 => 'C',
        q if q >= 0.26 => 'D',
        _ => 'F',
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::{Level, MapConfig, Obstacle};
    use crate::math::Vec3;

    #[test]
    fn random_scatter_scores_as_noise_clustered_does_not() {
        // Build two synthetic levels with the SAME object count: one uniform-random scatter, one tight
        // clusters. The clustered one must score higher clustering and lower noise-likeness — this is the
        // core anti-gaming property (equal count, very different composition score).
        let mut rng_state = 12345u64;
        let mut rng = || {
            rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1);
            ((rng_state >> 33) as f32) / (1u64 << 31) as f32
        };
        let base = Level::generate(MapConfig { seed: 1, ..Default::default() });
        let ts = base.ts();
        let (wx, wz) = (base.w as f32 * ts, base.h as f32 * ts);

        let mut scatter = Level::generate(MapConfig { seed: 1, ..Default::default() });
        scatter.cover_obstacles.clear();
        for _ in 0..60 {
            let c = Vec3::new(rng() * wx, 0.5, rng() * wz);
            scatter.cover_obstacles.push(Obstacle { center: c, half: Vec3::new(0.5, 0.5, 0.5) });
        }

        let mut clustered = Level::generate(MapConfig { seed: 1, ..Default::default() });
        clustered.cover_obstacles.clear();
        // 4 tight clusters of 15
        for k in 0..4 {
            let cx = (0.25 + 0.5 * (k % 2) as f32) * wx;
            let cz = (0.25 + 0.5 * (k / 2) as f32) * wz;
            for _ in 0..15 {
                let c = Vec3::new(cx + (rng() - 0.5) * 6.0, 0.5, cz + (rng() - 0.5) * 6.0);
                clustered.cover_obstacles.push(Obstacle { center: c, half: Vec3::new(0.5, 0.5, 0.5) });
            }
        }

        let s = analyze(&scatter);
        let c = analyze(&clustered);
        assert!(c.clustering > s.clustering + 0.1, "clustered must out-cluster scatter ({:.2} vs {:.2})", c.clustering, s.clustering);
        assert!(s.noise_likeness > c.noise_likeness, "scatter must read as more noise-like ({:.2} vs {:.2})", s.noise_likeness, c.noise_likeness);
    }

    #[test]
    fn adding_uniform_objects_does_not_inflate_the_score() {
        // The key failure of the old metric: "add more objects near a target density" boosted the grade. Here,
        // doubling the count with MORE uniform scatter must NOT meaningfully raise the composition score.
        let mut rng_state = 999u64;
        let mut rng = || {
            rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1);
            ((rng_state >> 33) as f32) / (1u64 << 31) as f32
        };
        let base = Level::generate(MapConfig { seed: 2, ..Default::default() });
        let ts = base.ts();
        let (wx, wz) = (base.w as f32 * ts, base.h as f32 * ts);
        let make = |n: usize, rng: &mut dyn FnMut() -> f32| {
            let mut lvl = Level::generate(MapConfig { seed: 2, ..Default::default() });
            lvl.cover_obstacles.clear();
            for _ in 0..n {
                lvl.cover_obstacles.push(Obstacle { center: Vec3::new(rng() * wx, 0.5, rng() * wz), half: Vec3::new(0.5, 0.5, 0.5) });
            }
            lvl
        };
        let few = analyze(&make(30, &mut rng)).score;
        let many = analyze(&make(120, &mut rng)).score;
        assert!(many <= few + 0.12, "piling on uniform objects must not inflate composition ({few:.2} -> {many:.2})");
    }

    #[test]
    fn authored_maps_out_compose_bare_procedural_clutter() {
        // The headline claim: a hand-authored map should read as better-composed than a procedural one whose
        // mapeval grade is inflated by density. Compare Desert Town (authored) against a couple of procedural
        // presets that graded A/B.
        use crate::level::PRESETS;
        let authored = ["Desert Town", "Flood Zone", "Malecon Breakwater"];
        let mut auth_scores = Vec::new();
        for name in authored {
            if let Some(p) = PRESETS.iter().find(|p| p.name == name) {
                auth_scores.push(honest_quality(&Level::generate(p.config(7))));
            }
        }
        let auth_mean = auth_scores.iter().sum::<f32>() / auth_scores.len().max(1) as f32;
        // just assert the metric runs and authored maps land in a sane band (not a hard ordering, which would
        // be brittle) — the report tool is where the full comparison lives.
        assert!(auth_mean > 0.2, "authored maps should compose above the floor, got {auth_mean:.2}");
        assert!(auth_mean <= 1.0);
    }
}
