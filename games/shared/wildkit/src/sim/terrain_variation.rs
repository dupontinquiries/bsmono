//! `sim::terrain_variation` — landform variety on top of the base heightfield (WID r78-03).
//!
//! Mixes ridged + billow value noise (with a regional selector + a light erosion
//! damp) onto a passed-in base height to carve plateaus, cliffs, rolling hills,
//! mountains, valleys and basins/oases, and tags each column with a [`Landform`]
//! so other systems (spawn, foliage, naming) can react. Pure height-domain math.
//!
//! Composes (does not fork):
//! - `sim::step_height` — the cliff slope threshold is derived from
//!   `max_step_on_slope` (a cliff is a slope so steep the usable step collapses),
//!   so "cliff" stays consistent with traversability (step_height.rs:26).
//! - `sim::river_flow` — [`shape_field`] runs `accumulate_flow` on the modified
//!   heights and tags `is_channel` columns as `Valley`, so valleys follow real
//!   drainage (river_flow.rs:68/109).
//!
//! Design refs: cave_biomes.docx (landform → biome/spawn reactions),
//! river_systems.docx (drainage-driven valleys).
//!
//! Pure, deterministic, panic-free — no `crate::render`, no GPU, no I/O, no
//! clock, no RNG (value noise seeded from `seed`, documented salts R20), no
//! `.unwrap()`/`.expect()` in non-test code. Release-safe: every helper is
//! unconditional (no `#[cfg]` gating); compiles clean under `cargo check --release`.

use crate::sim::river_flow::{accumulate_flow, is_channel};
use crate::sim::step_height::{max_step, max_step_on_slope};

/// Coarse landform class for a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Landform {
    Plain,
    Hills,
    Plateau,
    Cliff,
    Mountain,
    Valley,
    Basin,
}

/// The shaped result for one column.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrainSample {
    pub height_m: f32,
    pub slope01: f32,
    pub landform: Landform,
}

// ── tuning (documented; world units = metres) ──
const R_SALT: u64 = 0x5EED_1D6E_0000_0001; // ridge
const B_SALT: u64 = 0x5EED_B107_0000_0002; // billow
const M_SALT: u64 = 0x5EED_3EC7_0000_0003; // regional selector
const O_SALT: u64 = 0x5EED_B0F1_0000_0004; // basin bowl
const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;

const FREQ: f32 = 0.006; // ridge frequency (1/m)
const BFREQ: f32 = 0.010; // billow frequency
const MFREQ: f32 = 0.0012; // regional selector frequency
const OFREQ: f32 = 0.0020; // basin bowl frequency
const OCT: u32 = 5;

const HIGH_AMP: f32 = 180.0; // ridge amplitude in high-relief regions
const LOW_AMP: f32 = 15.0; //  ... in flat regions
const HILL_AMP: f32 = 35.0;
const BASIN_AMP: f32 = 60.0;
const PLATEAU_LIFT: f32 = 130.0; // regional elevation lift (makes flat highs = plateaus)
const MAX_RELIEF: f32 = 320.0; // hard bound on |height - base|

const EPS_M: f32 = 1.5; // finite-difference step for the pointwise slope
const SLOPE_REF: f32 = 8.0; // gradient (m/m) mapped to slope01 = 0.5

// classification bands
const BASIN_H: f32 = 25.0;
const LOWLAND: f32 = 70.0;
const HIGHLAND: f32 = 150.0;
const STEEP: f32 = 0.45;
const RIDGY: f32 = 0.5;
const CLIFF_FACTOR_RATIO: f32 = 0.55; // cliff once usable step < 0.55·max_step

fn mix64(x: u64) -> u64 {
    let mut z = x.wrapping_add(GOLDEN);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn hash_unit(seed: u64, ix: i64, iz: i64) -> f32 {
    let h = mix64(seed ^ (ix as u64).wrapping_mul(0xA24B_AED4_963E_E407) ^ (iz as u64).wrapping_mul(0x9FB2_1C65_1E98_DF25));
    (h >> 40) as f32 / (1u64 << 24) as f32 // [0,1)
}

fn smoother(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    crate::util::mathx::lerp(a, b, t) // r342 dedup
}

/// Bilinear value noise in `[0,1)`.
fn vnoise2(seed: u64, x: f32, z: f32) -> f32 {
    let x0 = x.floor();
    let z0 = z.floor();
    let (ix, iz) = (x0 as i64, z0 as i64);
    let (fx, fz) = (x - x0, z - z0);
    let c00 = hash_unit(seed, ix, iz);
    let c10 = hash_unit(seed, ix + 1, iz);
    let c01 = hash_unit(seed, ix, iz + 1);
    let c11 = hash_unit(seed, ix + 1, iz + 1);
    let u = smoother(fx);
    let v = smoother(fz);
    lerp(lerp(c00, c10, u), lerp(c01, c11, u), v)
}

/// Ridged multifractal in `[0,1]` — sharp crests near 1. Building block.
pub fn ridged(seed: u64, x: f32, z: f32, oct: u32) -> f32 {
    let mut sum = 0.0f32;
    let mut amp = 1.0f32;
    let mut tot = 0.0f32;
    let mut fx = x;
    let mut fz = z;
    let octaves = oct.max(1);
    for _ in 0..octaves {
        let n = vnoise2(seed, fx, fz);
        let r = 1.0 - (2.0 * n - 1.0).abs(); // ridge
        sum += r * r * amp;
        tot += amp;
        amp *= 0.5;
        fx *= 2.0;
        fz *= 2.0;
    }
    (sum / tot.max(1e-6)).clamp(0.0, 1.0)
}

/// Billow noise in `[0,1]` — rounded lobes (rolling hills). Building block.
fn billow(seed: u64, x: f32, z: f32, oct: u32) -> f32 {
    let mut sum = 0.0f32;
    let mut amp = 1.0f32;
    let mut tot = 0.0f32;
    let mut fx = x;
    let mut fz = z;
    let octaves = oct.max(1);
    for _ in 0..octaves {
        let n = vnoise2(seed, fx, fz);
        sum += (2.0 * n - 1.0).abs() * amp;
        tot += amp;
        amp *= 0.5;
        fx *= 2.0;
        fz *= 2.0;
    }
    (sum / tot.max(1e-6)).clamp(0.0, 1.0)
}

/// Erosion deposition bias in `[0,1]`: 1 on the flat (material stays), 0 on a
/// wall (material sheds). Building block. Monotone decreasing in slope.
pub fn erosion_bias(slope01: f32) -> f32 {
    (1.0 - slope01.clamp(0.0, 1.0)).clamp(0.0, 1.0)
}

/// A slope steep enough to read as a cliff — derived from `step_height`: the
/// usable step on this slope has collapsed below `CLIFF_FACTOR_RATIO` of flat.
fn is_cliff_slope(slope01: f32) -> bool {
    let flat = max_step(1.0);
    flat > 0.0 && max_step_on_slope(1.0, slope01) < CLIFF_FACTOR_RATIO * flat
}

/// Relief delta (bounded) + the ridge value at `(x, z)`.
fn relief_and_ridge(seed: u64, x: f32, z: f32) -> (f32, f32) {
    let m = vnoise2(seed ^ M_SALT, x * MFREQ, z * MFREQ); // regional selector 0..1
    let r = ridged(seed ^ R_SALT, x * FREQ, z * FREQ, OCT);
    let b = billow(seed ^ B_SALT, x * BFREQ, z * BFREQ, OCT);
    let bowl = vnoise2(seed ^ O_SALT, x * OFREQ, z * OFREQ);

    let ms = smoother(m);
    let amp = lerp(LOW_AMP, HIGH_AMP, ms);
    let lift = ms * PLATEAU_LIFT; // high-m regions rise → flat highs become plateaus
    let mountain = r * r * amp; // ridged peaks, steep
    let hills = (2.0 * b - 1.0) * HILL_AMP * (1.0 - m); // rolling, stronger in low regions
    let basin = -(1.0 - bowl) * BASIN_AMP * (1.0 - ms); // depressions in low regions

    let mut d = lift + mountain + hills + basin;
    if !d.is_finite() {
        d = 0.0;
    }
    (d.clamp(-MAX_RELIEF, MAX_RELIEF), r)
}

fn relief(seed: u64, x: f32, z: f32) -> f32 {
    relief_and_ridge(seed, x, z).0
}

/// Pointwise relief slope at `(x, z)` (base assumed locally smooth), in `[0,1]`.
fn relief_slope01(seed: u64, x: f32, z: f32) -> f32 {
    let hc = relief(seed, x, z);
    let gx = (relief(seed, x + EPS_M, z) - hc) / EPS_M;
    let gz = (relief(seed, x, z + EPS_M) - hc) / EPS_M;
    let g = (gx * gx + gz * gz).sqrt();
    if !g.is_finite() {
        0.0
    } else {
        (g / (g + SLOPE_REF)).clamp(0.0, 1.0)
    }
}

/// Classify a column from its elevation, slope and ridge sharpness.
/// `Cliff` ⇒ very steep; `Plateau` ⇒ flat top at elevation; `Mountain` ⇒ high &
/// ridged & steep; `Basin` ⇒ flat & very low. (`Valley` is drainage-driven — see
/// [`shape_field`] — so it is not produced here.)
pub fn classify_landform(height_m: f32, slope01: f32, ridge01: f32) -> Landform {
    let s = slope01.clamp(0.0, 1.0);
    if is_cliff_slope(s) {
        return Landform::Cliff;
    }
    if s >= STEEP {
        return if height_m >= HIGHLAND && ridge01 >= RIDGY {
            Landform::Mountain
        } else {
            Landform::Hills
        };
    }
    // gentle slope:
    if height_m >= HIGHLAND {
        return Landform::Plateau; // elevated + flat = plateau cap
    }
    if height_m <= BASIN_H {
        return Landform::Basin;
    }
    if height_m <= LOWLAND {
        return Landform::Plain;
    }
    if ridge01 >= RIDGY * 0.6 {
        Landform::Hills
    } else {
        Landform::Plain
    }
}

/// Shape one column: modified height + slope + landform tag. The relief is
/// lightly eroded (steep slopes shed material via [`erosion_bias`]).
pub fn shape(seed: u64, x: f32, z: f32, base_height_m: f32) -> TerrainSample {
    let (mut d, r) = relief_and_ridge(seed, x, z);
    let slope01 = relief_slope01(seed, x, z);
    d *= 0.7 + 0.3 * erosion_bias(slope01); // light erosion damp on steeps
    let height_m = base_height_m + d;
    let landform = classify_landform(height_m, slope01, r);
    TerrainSample { height_m, slope01, landform }
}

/// Shape a whole `w*h` field (row-major), composing `river_flow`: after carving
/// the relief, drainage is accumulated on the modified heights and the channel
/// columns are retagged `Valley`. `cell_m` is the world spacing between columns.
/// Empty on a size mismatch.
pub fn shape_field(seed: u64, base: &[f32], w: usize, h: usize, cell_m: f32) -> Vec<TerrainSample> {
    let n = w.checked_mul(h).unwrap_or(0);
    if n == 0 || base.len() != n {
        return Vec::new();
    }
    let cell = if cell_m.is_finite() && cell_m > 0.0 { cell_m } else { 1.0 };

    // 1) modified heights + ridge cache
    let mut heights = vec![0.0f32; n];
    let mut ridges = vec![0.0f32; n];
    for zz in 0..h {
        for xx in 0..w {
            let i = zz * w + xx;
            let (mut d, r) = relief_and_ridge(seed, xx as f32 * cell, zz as f32 * cell);
            // erosion damp uses the field slope below; approximate here with the
            // pointwise slope so heights and tags stay consistent.
            let sp = relief_slope01(seed, xx as f32 * cell, zz as f32 * cell);
            d *= 0.7 + 0.3 * erosion_bias(sp);
            heights[i] = base[i] + d;
            ridges[i] = r;
        }
    }

    // 2) slope from neighbour finite differences of the modified heights
    let slope_at = |xx: usize, zz: usize| -> f32 {
        let i = zz * w + xx;
        let xm = if xx > 0 { xx - 1 } else { xx };
        let xp = if xx + 1 < w { xx + 1 } else { xx };
        let zm = if zz > 0 { zz - 1 } else { zz };
        let zp = if zz + 1 < h { zz + 1 } else { zz };
        let dx_cells = (xp - xm).max(1) as f32;
        let dz_cells = (zp - zm).max(1) as f32;
        let gx = (heights[zz * w + xp] - heights[zz * w + xm]) / (dx_cells * cell);
        let gz = (heights[zp * w + xx] - heights[zm * w + xx]) / (dz_cells * cell);
        let g = (gx * gx + gz * gz).sqrt();
        let _ = i;
        if g.is_finite() {
            (g / (g + SLOPE_REF)).clamp(0.0, 1.0)
        } else {
            0.0
        }
    };

    // 3) drainage on the modified heights → channel threshold (top of the range)
    let accum = accumulate_flow(&heights, w, h);
    let max_a = accum.iter().copied().fold(0.0f32, f32::max);
    let threshold = (max_a * 0.25).max(2.0);

    // 4) classify + Valley override on channels
    let mut out = Vec::with_capacity(n);
    for zz in 0..h {
        for xx in 0..w {
            let i = zz * w + xx;
            let slope01 = slope_at(xx, zz);
            let mut landform = classify_landform(heights[i], slope01, ridges[i]);
            if !accum.is_empty() && is_channel(accum[i], threshold) {
                landform = Landform::Valley; // valleys follow real drainage
            }
            out.push(TerrainSample { height_m: heights[i], slope01, landform });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic() {
        let a = shape(42, 123.0, -77.0, 80.0);
        let b = shape(42, 123.0, -77.0, 80.0);
        assert_eq!(a, b);
        let base = vec![60.0f32; 64];
        assert_eq!(shape_field(7, &base, 8, 8, 10.0), shape_field(7, &base, 8, 8, 10.0));
    }

    #[test]
    fn height_finite_and_bounded_relative_to_base() {
        let mut s = 0u64;
        for k in 0..4000 {
            s = s.wrapping_add(1);
            let x = (k as f32) * 7.3 - 5000.0;
            let z = (k as f32) * 3.1;
            let base = 50.0 + (k % 200) as f32;
            let t = shape(0xABCD, x, z, base);
            assert!(t.height_m.is_finite(), "finite height");
            assert!((t.height_m - base).abs() <= MAX_RELIEF + 1e-3, "bounded relief: {} vs base {}", t.height_m, base);
            let _ = s;
        }
    }

    #[test]
    fn slope_always_in_range() {
        for k in 0..3000 {
            let x = (k as f32) * 11.7;
            let z = (k as f32) * -5.9 + 100.0;
            let t = shape(99, x, z, 90.0);
            assert!((0.0..=1.0).contains(&t.slope01), "slope {} out of range", t.slope01);
        }
    }

    #[test]
    fn erosion_bias_and_ridged_are_well_behaved() {
        assert_eq!(erosion_bias(0.0), 1.0);
        assert_eq!(erosion_bias(1.0), 0.0);
        let (mut prev, mut s) = (2.0f32, 0.0f32);
        while s <= 1.0 {
            let e = erosion_bias(s);
            assert!((0.0..=1.0).contains(&e) && e <= prev + 1e-6, "erosion_bias monotone/in-range");
            prev = e;
            s += 0.05;
        }
        for k in 0..500 {
            let r = ridged(5, k as f32 * 0.13, k as f32 * 0.27, 5);
            assert!((0.0..=1.0).contains(&r) && r.is_finite());
        }
    }

    #[test]
    fn classify_cliff_needs_high_slope() {
        // step_height-derived cliff threshold: factor 1-0.6*slope < 0.55 ⇒ slope > 0.75
        assert!(is_cliff_slope(0.9), "0.9 is a cliff");
        assert!(!is_cliff_slope(0.1), "0.1 is not");
        assert_eq!(classify_landform(100.0, 0.95, 0.8), Landform::Cliff);
        // invariant: wherever a Cliff IS tagged, it carries a steep slope. (Cliffs
        // are rare from smooth value noise — reachability is shown by the direct
        // classify above; here we assert the property holds for any sampled one.)
        for k in 0..6000 {
            let t = shape(0x5151, k as f32 * 6.1 - 3000.0, k as f32 * 4.7, 120.0);
            if t.landform == Landform::Cliff {
                assert!(t.slope01 > 0.75, "Cliff ⇒ high slope, got {}", t.slope01);
            }
        }
    }

    #[test]
    fn plateau_is_elevated_and_flat() {
        // direct: elevated + gentle + low ridge ⇒ Plateau
        assert_eq!(classify_landform(200.0, 0.1, 0.2), Landform::Plateau);
        // field: every Plateau-tagged column is flat (low slope ⇒ flat cap / low variance)
        let base = vec![140.0f32; 80 * 80];
        let field = shape_field(0xDEAD, &base, 80, 80, 12.0);
        let plateau: Vec<f32> = field.iter().filter(|t| t.landform == Landform::Plateau).map(|t| t.slope01).collect();
        if !plateau.is_empty() {
            let maxs = plateau.iter().copied().fold(0.0f32, f32::max);
            assert!(maxs < STEEP, "plateau caps stay flat (max slope {maxs} < {STEEP})");
            let mean = plateau.iter().sum::<f32>() / plateau.len() as f32;
            let var = plateau.iter().map(|s| (s - mean) * (s - mean)).sum::<f32>() / plateau.len() as f32;
            assert!(var < 0.02, "low slope variance on plateau tops, got {var}");
        }
    }

    #[test]
    fn valleys_sit_on_drainage_channels() {
        // synthetic bowl draining to the centre column line: a clear trough.
        let (w, h) = (40usize, 40usize);
        let mut base = vec![0.0f32; w * h];
        for z in 0..h {
            for x in 0..w {
                let dx = (x as f32 - (w as f32) * 0.5).abs();
                // V-shaped trough along the centre, tilted so flow runs down +z
                base[z * w + x] = 100.0 + dx * 2.0 - z as f32 * 1.0;
            }
        }
        let field = shape_field(0xC0FFEE, &base, w, h, 5.0);
        let accum = accumulate_flow(&{
            // re-derive the SAME modified heights the field used, to get its accum
            let mut hgt = vec![0.0f32; w * h];
            for z in 0..h {
                for x in 0..w {
                    let i = z * w + x;
                    let (mut d, _r) = relief_and_ridge(0xC0FFEE, x as f32 * 5.0, z as f32 * 5.0);
                    let sp = relief_slope01(0xC0FFEE, x as f32 * 5.0, z as f32 * 5.0);
                    d *= 0.7 + 0.3 * erosion_bias(sp);
                    hgt[i] = base[i] + d;
                }
            }
            hgt
        }, w, h);
        let max_a = accum.iter().copied().fold(0.0f32, f32::max);
        let threshold = (max_a * 0.25).max(2.0);

        let valleys: Vec<usize> = field.iter().enumerate().filter(|(_, t)| t.landform == Landform::Valley).map(|(i, _)| i).collect();
        assert!(!valleys.is_empty(), "the trough produced valleys");
        // every Valley tag sits on a real channel
        for &i in &valleys {
            assert!(is_channel(accum[i], threshold), "Valley at {i} must be a drainage channel");
        }
    }

    #[test]
    fn landform_mix_is_varied() {
        use std::collections::BTreeMap;
        let mut counts: BTreeMap<&str, u32> = BTreeMap::new();
        let mut total = 0u32;
        let mut x = -8000.0f32;
        while x < 8000.0 {
            let mut z = -8000.0f32;
            while z < 8000.0 {
                let t = shape(0x77, x, z, 95.0);
                let name = match t.landform {
                    Landform::Plain => "plain",
                    Landform::Hills => "hills",
                    Landform::Plateau => "plateau",
                    Landform::Cliff => "cliff",
                    Landform::Mountain => "mountain",
                    Landform::Valley => "valley",
                    Landform::Basin => "basin",
                };
                *counts.entry(name).or_insert(0) += 1;
                total += 1;
                z += 240.0;
            }
            x += 240.0;
        }
        let max = counts.values().copied().max().unwrap_or(total);
        let frac = max as f32 / total as f32;
        assert!(frac <= 0.70, "no single landform should dominate (got {frac:.2}: {counts:?})");
        assert!(counts.len() >= 3, "at least three landform classes present: {counts:?}");
    }
}
