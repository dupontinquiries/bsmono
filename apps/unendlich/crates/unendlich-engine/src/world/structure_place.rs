//! `structure_place` — deterministic structure-placement scoring. **OPTIONAL.**
//!
//! Worker-03 / WORLD-PERF pure core, round 2. Given per-cell terrain features
//! (flatness, normalized elevation, distance-from-water) and a structure's
//! preferences, score every cell for suitability and return the top ranked
//! candidate sites. Deterministic and property-testable (no visuals): the
//! terrain generator / structure pass feeds features in and gets ranked,
//! in-bounds sites out.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Normalized per-cell terrain features (each in [0,1]; clamped on use).
#[derive(Clone, Copy, Debug)]
pub struct SiteFeatures {
    /// 1.0 = perfectly flat, 0.0 = steep.
    pub flatness: f32,
    /// Normalized elevation (0 = lowest, 1 = highest).
    pub elevation: f32,
    /// Distance from water (0 = at the shoreline, 1 = far inland).
    pub water_dist: f32,
}

/// What a structure type prefers.
#[derive(Clone, Copy, Debug)]
pub struct StructurePrefs {
    pub w_flat: f32,
    pub w_elev: f32,
    pub w_water: f32,
    pub pref_elevation: f32,
    pub pref_water_dist: f32,
}

impl Default for StructurePrefs {
    fn default() -> Self {
        // A generic building: strongly prefers flat ground, mid elevation, a
        // little back from the water.
        StructurePrefs {
            w_flat: 2.0,
            w_elev: 1.0,
            w_water: 1.0,
            pref_elevation: 0.4,
            pref_water_dist: 0.3,
        }
    }
}

/// Suitability of a single cell, in [0,1]. Increasing in flatness; for
/// elevation and water distance, peaks at the preferred value and falls off
/// linearly with the absolute difference.
pub fn suitability(f: &SiteFeatures, p: &StructurePrefs) -> f32 {
    let flat = clamp01(f.flatness);
    let elev_fit = 1.0 - (clamp01(f.elevation) - clamp01(p.pref_elevation)).abs();
    let water_fit = 1.0 - (clamp01(f.water_dist) - clamp01(p.pref_water_dist)).abs();
    let wf = p.w_flat.max(0.0);
    let we = p.w_elev.max(0.0);
    let ww = p.w_water.max(0.0);
    let wsum = (wf + we + ww).max(1e-6);
    clamp01((wf * flat + we * elev_fit + ww * water_fit) / wsum)
}

/// A scored candidate site.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Candidate {
    pub x: u32,
    pub y: u32,
    pub score: f32,
}

/// Score a `width × height` (row-major) feature grid and return the top
/// `top_k` sites, sorted by descending score with a deterministic
/// (row, then column) tiebreak. All returned sites are within bounds.
pub fn rank_sites(
    features: &[SiteFeatures],
    width: u32,
    height: u32,
    prefs: &StructurePrefs,
    top_k: usize,
) -> Vec<Candidate> {
    let w = width as usize;
    let h = height as usize;
    let mut cands: Vec<Candidate> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            if idx >= features.len() {
                break;
            }
            cands.push(Candidate {
                x: x as u32,
                y: y as u32,
                score: suitability(&features[idx], prefs),
            });
        }
    }
    // Scores are clamped to [0,1] (finite), so this is a total order.
    cands.sort_by(|a, b| {
        if a.score > b.score {
            std::cmp::Ordering::Less // higher score ranks first
        } else if a.score < b.score {
            std::cmp::Ordering::Greater
        } else {
            a.y.cmp(&b.y).then(a.x.cmp(&b.x))
        }
    });
    cands.truncate(top_k);
    cands
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feat(flat: f32, elev: f32, water: f32) -> SiteFeatures {
        SiteFeatures { flatness: flat, elevation: elev, water_dist: water }
    }

    // Deterministic feature grid (no rng).
    fn gen_grid(w: u32, h: u32, seed: u64) -> Vec<SiteFeatures> {
        let mut v = Vec::with_capacity((w * h) as usize);
        for i in 0..(w * h) as u64 {
            let a = i.wrapping_mul(2654435761).wrapping_add(seed.wrapping_mul(40503));
            let f = ((a >> 8) % 1000) as f32 / 1000.0;
            let e = ((a >> 18) % 1000) as f32 / 1000.0;
            let wd = ((a >> 28) % 1000) as f32 / 1000.0;
            v.push(feat(f, e, wd));
        }
        v
    }

    #[test]
    fn prop_suitability_bounded() {
        let p = StructurePrefs::default();
        for grid_seed in 0..8u64 {
            for f in gen_grid(8, 8, grid_seed) {
                let s = suitability(&f, &p);
                assert!(s >= 0.0 && s <= 1.0 && s.is_finite());
            }
        }
    }

    #[test]
    fn prop_score_monotone_in_flatness() {
        let p = StructurePrefs::default();
        // Fix elevation/water at the preferred values; raise flatness.
        let mut prev = suitability(&feat(0.0, p.pref_elevation, p.pref_water_dist), &p);
        for i in 1..=20 {
            let flat = i as f32 / 20.0;
            let cur = suitability(&feat(flat, p.pref_elevation, p.pref_water_dist), &p);
            assert!(cur >= prev - 1e-6, "suitability not monotone in flatness");
            prev = cur;
        }
    }

    #[test]
    fn prop_sites_within_bounds() {
        let p = StructurePrefs::default();
        let (w, h) = (12u32, 9u32);
        let grid = gen_grid(w, h, 5);
        let sites = rank_sites(&grid, w, h, &p, 16);
        for c in &sites {
            assert!(c.x < w && c.y < h, "site out of bounds");
        }
    }

    #[test]
    fn prop_top_k_respected_and_sorted() {
        let p = StructurePrefs::default();
        let (w, h) = (10u32, 10u32);
        let grid = gen_grid(w, h, 11);
        let k = 7;
        let sites = rank_sites(&grid, w, h, &p, k);
        assert!(sites.len() <= k);
        for win in sites.windows(2) {
            assert!(win[0].score >= win[1].score, "not sorted descending");
        }
    }

    #[test]
    fn prop_deterministic() {
        let p = StructurePrefs::default();
        let (w, h) = (14u32, 6u32);
        let grid = gen_grid(w, h, 99);
        let a = rank_sites(&grid, w, h, &p, 10);
        let b = rank_sites(&grid, w, h, &p, 10);
        assert_eq!(a, b);
    }

    #[test]
    fn best_site_is_the_most_suitable() {
        let p = StructurePrefs::default();
        // One clearly-ideal cell among mediocre ones.
        let mut grid = vec![feat(0.1, 0.9, 0.0); 9]; // poor: steep, wrong elev, at water
        grid[5] = feat(1.0, p.pref_elevation, p.pref_water_dist); // ideal
        let sites = rank_sites(&grid, 3, 3, &p, 9);
        assert_eq!((sites[0].x, sites[0].y), (2, 1)); // idx 5 → (x=2,y=1)
    }
}
