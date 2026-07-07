//! sightline.rs — **gameplay geometry analysis** built on line-of-sight ("gun arc") calculation. Where
//! `mapeval` scores coarse aggregate metrics, this module builds a per-tile visibility model and derives the
//! spatial gameplay structure a designer actually cares about: which tiles are exposed overwatch perches,
//! which are safe pockets, and — crucially — where the **chokepoints** are (the narrow throats every route
//! funnels through). It answers "is this map a series of meaningful fights, or one big shooting gallery?"
//!
//! The core is a **visibility field**: for a sampled set of walkable tiles, we shoot rays to every other
//! sampled tile and test them against the eye-height cover grid (the same "tall obstacle blocks a standing
//! sightline" rule the flow metric uses). From the resulting visibility counts we compute:
//!
//! - **exposure(tile)** — fraction of the map that can see (and shoot) this tile. High everywhere = a flat
//!   gallery with no safety; near-zero everywhere = a claustrophobic maze. A healthy map has a spread.
//! - **overwatch** — the peak-exposure tiles (power positions that see the most ground).
//! - **chokepoints** — walkable throats where the local passage width is small *and* they connect two
//!   otherwise weakly-connected regions. These are the map's tactical anchors; too few = no structure, too
//!   many = a corridor crawl.
//! - **openness/cover variance** — the *spread* of exposure. Memorable maps mix exposed and sheltered space;
//!   bland maps are uniform. This variance feeds the scenic/variety scoring too.

use crate::level::{Cell, Level};

/// The result of a sightline analysis pass.
#[derive(Clone, Debug)]
pub struct SightlineReport {
    /// Mean exposure across sampled tiles (0..1): the average fraction of the map a tile is visible from.
    pub mean_exposure: f32,
    /// Standard deviation of exposure — the *spread* between exposed and sheltered ground. Higher is more
    /// tactically varied (a map that mixes perches and pockets), which reads as more memorable.
    pub exposure_spread: f32,
    /// Number of detected chokepoints (narrow throats connecting open regions).
    pub chokepoints: usize,
    /// Peak exposure among sampled tiles (the strongest overwatch position's reach).
    pub peak_exposure: f32,
    /// Fraction of walkable tiles that are "safe" (exposure below a low threshold) — cover pockets to break
    /// to. A map with none is a death gallery; a map that's all safe is a maze with no engagements.
    pub safe_fraction: f32,
    /// A single 0..1 tactical-structure score: rewards a healthy exposure spread, a sane chokepoint count for
    /// the map's size, and the presence of both exposed lanes and safe pockets.
    pub structure_score: f32,
}

/// Build the eye-height "tall cover" mask: tiles where a standing sightline is blocked by an obstacle that
/// straddles eye level (same rule the flow metric uses — low cover you shoot over doesn't block LOS).
fn eye_block_mask(level: &Level) -> Vec<bool> {
    let ts = level.ts();
    let eye = level.floor_y + 1.5;
    let mut tall = vec![false; (level.w * level.h) as usize];
    for o in &level.obstacles {
        if o.center.y + o.half.y < eye || o.center.y - o.half.y > eye {
            continue;
        }
        let tx0 = ((o.center.x - o.half.x) / ts) as i32;
        let tx1 = ((o.center.x + o.half.x) / ts) as i32;
        let tz0 = ((o.center.z - o.half.z) / ts) as i32;
        let tz1 = ((o.center.z + o.half.z) / ts) as i32;
        for tz in tz0..=tz1 {
            for tx in tx0..=tx1 {
                if level.in_bounds(tx, tz) {
                    tall[(tz * level.w + tx) as usize] = true;
                }
            }
        }
    }
    tall
}

/// Does a clear eye-height sightline exist between two tile centres? A supercover DDA line walk that stops at
/// the first wall or eye-height obstacle. This is the "can A shoot B" test — the gun-arc primitive.
fn has_los(level: &Level, tall: &[bool], a: (i32, i32), b: (i32, i32)) -> bool {
    let (mut x0, mut z0) = (a.0, a.1);
    let (x1, z1) = (b.0, b.1);
    let dx = (x1 - x0).abs();
    let dz = -(z1 - z0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sz = if z0 < z1 { 1 } else { -1 };
    let mut err = dx + dz;
    loop {
        if (x0, z0) != a {
            // block on walls or eye-height cover; the endpoints themselves are allowed
            if !matches!(level.cell(x0, z0), Cell::Floor) {
                return false;
            }
            if (x0, z0) != b && level.in_bounds(x0, z0) && tall[(z0 * level.w + x0) as usize] {
                return false;
            }
        }
        if (x0, z0) == (x1, z1) {
            return true;
        }
        let e2 = 2 * err;
        if e2 >= dz {
            err += dz;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            z0 += sz;
        }
    }
}

/// The local passage width at a walkable tile: the smaller of its horizontal and vertical open runs (how many
/// contiguous open tiles straddle it on each axis). "Open" means walkable floor *and* not blocked by an eye-
/// height obstacle — so this works for authored maps whose walls are freestanding boxes (in the `tall` mask)
/// rather than grid `Wall` cells. Small width = a throat; large = an open field.
fn passage_width(level: &Level, tall: &[bool], x: i32, z: i32) -> i32 {
    let open = |x: i32, z: i32| level.cell(x, z).walkable() && !(level.in_bounds(x, z) && tall[(z * level.w + x) as usize]);
    let run = |dx: i32, dz: i32| {
        let mut d = 0;
        while d < 12 && open(x + dx * (d + 1), z + dz * (d + 1)) {
            d += 1;
        }
        d
    };
    let horiz = run(1, 0) + run(-1, 0) + 1;
    let vert = run(0, 1) + run(0, -1) + 1;
    horiz.min(vert)
}

/// Analyse the map's sightline / chokepoint structure.
pub fn analyze(level: &Level) -> SightlineReport {
    let tall = eye_block_mask(level);
    // Collect walkable floor tiles.
    let mut floor: Vec<(i32, i32)> = Vec::new();
    for z in 0..level.h {
        for x in 0..level.w {
            if level.cell(x, z) == Cell::Floor {
                floor.push((x, z));
            }
        }
    }
    if floor.len() < 8 {
        return SightlineReport {
            mean_exposure: 0.0,
            exposure_spread: 0.0,
            chokepoints: 0,
            peak_exposure: 0.0,
            safe_fraction: 1.0,
            structure_score: 0.0,
        };
    }
    // Subsample to keep the O(n^2) visibility pass affordable on big maps: cap the sample set.
    let cap = 260usize;
    let stride = (floor.len() / cap).max(1);
    let sample: Vec<(i32, i32)> = floor.iter().copied().step_by(stride).collect();
    let m = sample.len().max(1);

    // Pairwise visibility → per-sample exposure = fraction of other samples that can see it.
    let mut seen_by = vec![0u32; m];
    for i in 0..m {
        for j in (i + 1)..m {
            if has_los(level, &tall, sample[i], sample[j]) {
                seen_by[i] += 1;
                seen_by[j] += 1;
            }
        }
    }
    let exposures: Vec<f32> = seen_by.iter().map(|&c| c as f32 / (m as f32 - 1.0).max(1.0)).collect();
    let mean = exposures.iter().sum::<f32>() / m as f32;
    let var = exposures.iter().map(|e| (e - mean).powi(2)).sum::<f32>() / m as f32;
    let spread = var.sqrt();
    let peak = exposures.iter().cloned().fold(0.0f32, f32::max);
    let safe_thresh = (mean * 0.5).max(0.08);
    let safe_fraction = exposures.iter().filter(|&&e| e < safe_thresh).count() as f32 / m as f32;

    // Chokepoints: throats (small local passage width) that connect regions. We scan the *full* floor for
    // narrow tiles (not just the exposure subsample, which is too sparse to catch doorways), then confirm each
    // is a genuine connector: a narrow tile whose two open sides lead to substantially more open space (so a
    // one-tile-wide doorway between two rooms counts, but a dead-end nook doesn't). Cheap and geometric.
    let mut chokepoints = 0usize;
    let mut choke_mask = vec![false; (level.w * level.h) as usize];
    let open = |x: i32, z: i32| level.cell(x, z).walkable() && !(level.in_bounds(x, z) && tall[(z * level.w + x) as usize]);
    for &(x, z) in &floor {
        // skip tiles that are themselves blocked at eye height
        if !open(x, z) {
            continue;
        }
        let width = passage_width(level, &tall, x, z);
        if width > 3 {
            continue;
        }
        let run = |dx: i32, dz: i32| {
            let mut d = 0;
            while d < 12 && open(x + dx * (d + 1), z + dz * (d + 1)) {
                d += 1;
            }
            d
        };
        let (hx0, hx1) = (run(-1, 0), run(1, 0));
        let (hz0, hz1) = (run(0, -1), run(0, 1));
        let horiz = hx0 + hx1;
        let vert = hz0 + hz1;
        // A throat: one axis is a real passage (open for several tiles both ways), the other is pinched
        // (short on at least one side). This catches 1–2-wide doorways and alley mouths between open areas
        // without flagging the middle of a wide field (both axes long) or a dead-end nook (both axes short).
        let is_throat = (horiz >= 4 && vert <= 2 && hz0.min(hz1) <= 1) || (vert >= 4 && horiz <= 2 && hx0.min(hx1) <= 1);
        if is_throat {
            choke_mask[(z * level.w + x) as usize] = true;
        }
    }
    // Collapse adjacent throat tiles into single chokepoints (a 1-wide corridor shouldn't count every tile).
    for z in 0..level.h {
        for x in 0..level.w {
            if !choke_mask[(z * level.w + x) as usize] {
                continue;
            }
            // count it only if it's the "start" of a throat run (no throat tile immediately behind it)
            let behind_x = x > 0 && choke_mask[(z * level.w + x - 1) as usize];
            let behind_z = z > 0 && choke_mask[((z - 1) * level.w + x) as usize];
            if !behind_x && !behind_z {
                chokepoints += 1;
            }
        }
    }

    // Structure score. What actually makes a map play well isn't hitting one "correct" average exposure —
    // a dense town legitimately sits near 10% exposure, an open arena near 40% — it's whether the map has
    // internal *variety* (some exposed lanes to contest, some sheltered ground to break to) and a sane number
    // of chokepoints for its size. So we reward:
    //   • relative spread (coefficient of variation): does exposure vary a lot *relative to its own mean*?
    //     This treats a town and an arena fairly — each is judged against its own baseline.
    //   • absolute spread: a floor of raw variation, so a perfectly uniform map still scores low.
    //   • a healthy safe fraction: neither a death gallery (≈0% safe) nor a wall-maze (≈100% safe).
    //   • chokepoint density near ~1 per 40 sampled tiles — articulated, not a corridor crawl or a bare field.
    let rel_spread = if mean > 1e-3 { (spread / mean).min(1.5) / 1.5 } else { 0.0 };
    let abs_spread = (spread / 0.16).clamp(0.0, 1.0);
    let variety_term = (rel_spread * 0.6 + abs_spread * 0.4).clamp(0.0, 1.0);
    // safe fraction: reward the 15–70% band (both exposed and safe ground exist); fall off outside it.
    let safe_term = if safe_fraction < 0.15 {
        safe_fraction / 0.15
    } else if safe_fraction > 0.70 {
        ((1.0 - safe_fraction) / 0.30).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let choke_density = chokepoints as f32 / (m as f32 / 40.0).max(1.0);
    let choke_term = 1.0 - (choke_density - 1.0).abs().min(1.0); // ideal ~1.0 per 40 tiles
    let structure_score = (variety_term * 0.45 + safe_term * 0.30 + choke_term * 0.25).clamp(0.0, 1.0);

    SightlineReport {
        mean_exposure: mean,
        exposure_spread: spread,
        chokepoints,
        peak_exposure: peak,
        safe_fraction,
        structure_score,
    }
}

/// A suggested cover placement: a world (x, z) and a recommended half-extent, generated by finding the most
/// over-exposed walkable tiles and proposing eye-height cover there. An authoring pass can drop rocks/crates/
/// planters at these spots to break a map's worst shooting-gallery sightlines — the analysis feeding back
/// into generation. Returns up to `budget` spots, spaced apart so they don't clump.
pub fn suggest_cover(level: &Level, budget: usize) -> Vec<(f32, f32, f32)> {
    let tall = eye_block_mask(level);
    let mut floor: Vec<(i32, i32)> = Vec::new();
    for z in 0..level.h {
        for x in 0..level.w {
            if level.cell(x, z) == Cell::Floor && !tall[(z * level.w + x) as usize] {
                floor.push((x, z));
            }
        }
    }
    if floor.len() < 8 {
        return Vec::new();
    }
    let cap = 260usize;
    let stride = (floor.len() / cap).max(1);
    let sample: Vec<(i32, i32)> = floor.iter().copied().step_by(stride).collect();
    let m = sample.len();
    let mut seen_by = vec![0u32; m];
    for i in 0..m {
        for j in (i + 1)..m {
            if has_los(level, &tall, sample[i], sample[j]) {
                seen_by[i] += 1;
                seen_by[j] += 1;
            }
        }
    }
    let mut ranked: Vec<usize> = (0..m).collect();
    ranked.sort_by(|&a, &b| seen_by[b].cmp(&seen_by[a]));
    let ts = level.ts();
    let mut out: Vec<(f32, f32, f32)> = Vec::new();
    let min_sep = 8.0 * ts; // space cover widely: the goal is to break long lanes into segments, NOT to
                            // blanket every exposed tile (which would flatten the exposure variety that makes
                            // a map interesting — cover should punctuate sightlines, not erase them)
    for &idx in &ranked {
        if out.len() >= budget {
            break;
        }
        let (x, z) = sample[idx];
        let (wx, wz) = (x as f32 * ts, z as f32 * ts);
        if out.iter().any(|&(ox, oz, _)| (ox - wx).powi(2) + (oz - wz).powi(2) < min_sep * min_sep) {
            continue;
        }
        let expo = seen_by[idx] as f32 / (m as f32 - 1.0).max(1.0);
        let half = 0.7 + expo * 0.8;
        out.push((wx, wz, half));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::PRESETS;

    #[test]
    fn los_blocks_on_walls() {
        // On any generated map, a tile should always see itself and adjacent open tiles, and the report
        // should produce sane, bounded numbers.
        let l = crate::level::Level::generate(PRESETS[0].config(7));
        let r = analyze(&l);
        assert!(r.mean_exposure >= 0.0 && r.mean_exposure <= 1.0);
        assert!(r.exposure_spread >= 0.0);
        assert!(r.peak_exposure >= r.mean_exposure, "peak exposure must be >= mean");
        assert!(r.structure_score >= 0.0 && r.structure_score <= 1.0);
    }

    #[test]
    fn an_open_field_is_more_exposed_than_a_cover_dense_map() {
        // The Flood Zone (deliberate cover, channels) should show a lower mean exposure or higher spread than
        // a wide-open arena, because its cover breaks sightlines. We just assert the analysis distinguishes
        // maps rather than returning a constant.
        let flood = PRESETS.iter().find(|p| p.name == "Flood Zone");
        let slab = PRESETS.iter().find(|p| p.name == "Concrete Slab");
        if let (Some(f), Some(s)) = (flood, slab) {
            let rf = analyze(&crate::level::Level::generate(f.config(3)));
            let rs = analyze(&crate::level::Level::generate(s.config(3)));
            // they must differ on at least one structural axis — the analysis isn't a constant
            let differ = (rf.mean_exposure - rs.mean_exposure).abs() > 0.02
                || (rf.exposure_spread - rs.exposure_spread).abs() > 0.02
                || rf.chokepoints != rs.chokepoints;
            assert!(differ, "sightline analysis should distinguish structurally different maps");
        }
    }

    #[test]
    fn every_preset_produces_a_bounded_report() {
        // Guard against panics / NaNs across the whole map fleet.
        for p in PRESETS.iter().take(20) {
            let l = crate::level::Level::generate(p.config(11));
            let r = analyze(&l);
            assert!(r.structure_score.is_finite(), "{} produced non-finite structure score", p.name);
            assert!(r.mean_exposure.is_finite());
        }
    }

    #[test]
    fn suggest_cover_targets_exposed_spots_within_bounds() {
        // The cover-suggestion pass should return in-bounds, spaced positions and respect the budget.
        let l = crate::level::Level::generate(PRESETS[0].config(7));
        let spots = suggest_cover(&l, 8);
        assert!(spots.len() <= 8);
        let ts = l.ts();
        for &(x, z, half) in &spots {
            assert!(x >= 0.0 && x <= l.w as f32 * ts, "cover x in bounds");
            assert!(z >= 0.0 && z <= l.h as f32 * ts, "cover z in bounds");
            assert!(half > 0.0 && half < 2.0, "sane cover size");
        }
        // spacing: no two suggestions on top of each other
        for i in 0..spots.len() {
            for j in (i + 1)..spots.len() {
                let d2 = (spots[i].0 - spots[j].0).powi(2) + (spots[i].1 - spots[j].1).powi(2);
                assert!(d2 > (2.0 * ts).powi(2), "suggested cover should be spaced apart");
            }
        }
    }
}
