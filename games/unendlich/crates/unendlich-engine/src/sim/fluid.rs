//! Liquids — turning blocky water into a flowing, floating, tinted medium.
//!
//! Three deterministic, integer/[`Fixed`] layers (the "fancy shader" that draws
//! the field as a smooth surface is the only non-CPU part):
//!
//! 1. **Field** ([`FluidField`]): a coarse per-cell water amount (0..=[`MAX_WATER`]).
//!    [`flow_step`] settles it under gravity and spreads it laterally, conserving
//!    total volume — the "low-res probabilistic voxel" liquid the renderer skins.
//! 2. **Forces** ([`buoyancy_accel`], [`submerged_fraction`], [`fluid_drag`]):
//!    a body lighter than the fluid rises, heavier sinks, equal hovers; motion
//!    is damped. Plugs into `step_character` when a body is in fluid.
//! 3. **Look data** ([`water_color`]): depth + turbidity (suspended dirt) → the
//!    tint the surface shader applies. Deeper + dirtier = darker and murkier.

use super::fixed::Fixed;

/// Per-cell fill: 0 = empty, [`MAX_WATER`] = a full water block. A coarse scale
/// keeps the field tiny (one byte per cell) and the flow integer-exact.
pub const MAX_WATER: u8 = 8;

/// A coarse 3D water-amount field.
#[derive(Clone, PartialEq, Debug)]
pub struct FluidField {
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    level: Vec<u8>,
}

impl FluidField {
    pub fn new(nx: usize, ny: usize, nz: usize) -> Self {
        Self { nx, ny, nz, level: vec![0; nx * ny * nz] }
    }
    #[inline]
    fn idx(&self, x: usize, y: usize, z: usize) -> usize {
        x + self.nx * (y + self.ny * z)
    }
    #[inline]
    pub fn get(&self, x: usize, y: usize, z: usize) -> u8 {
        self.level[self.idx(x, y, z)]
    }
    #[inline]
    pub fn set(&mut self, x: usize, y: usize, z: usize, v: u8) {
        let i = self.idx(x, y, z);
        self.level[i] = v.min(MAX_WATER);
    }
    /// Total water volume (sum of all cell levels) — the conserved quantity.
    pub fn total(&self) -> u32 {
        self.level.iter().map(|&l| l as u32).sum()
    }
    /// Highest cell index with any water in column `(x, z)`, or `None` if dry.
    pub fn surface_y(&self, x: usize, z: usize) -> Option<usize> {
        (0..self.ny).rev().find(|&y| self.get(x, y, z) > 0)
    }
}

/// One flow tick: gravity settle (water falls to the bottom of each column,
/// skipping solid cells) followed by lateral spread (a fuller cell pushes one
/// unit to a strictly-lower same-height neighbor). Total volume is conserved.
/// Deterministic: fixed iteration order, no randomness.
pub fn flow_step(f: &FluidField, is_solid: impl Fn(usize, usize, usize) -> bool) -> FluidField {
    let mut out = f.clone();

    // --- Gravity: settle water within each vertical run of non-solid cells.
    //     A solid cell partitions the column, so water resting on a shelf can't
    //     fall through it. Each segment pools its water and refills bottom-up. ---
    for z in 0..out.nz {
        for x in 0..out.nx {
            let mut y = 0;
            while y < out.ny {
                if is_solid(x, y, z) {
                    y += 1;
                    continue;
                }
                // Non-solid segment starts here; gather it.
                let seg_start = y;
                let mut water: u32 = 0;
                while y < out.ny && !is_solid(x, y, z) {
                    water += out.get(x, y, z) as u32;
                    out.set(x, y, z, 0);
                    y += 1;
                }
                let seg_end = y; // exclusive
                // Refill from the segment's bottom up.
                let mut yy = seg_start;
                while water > 0 && yy < seg_end {
                    let put = water.min(MAX_WATER as u32) as u8;
                    out.set(x, yy, z, put);
                    water -= put as u32;
                    yy += 1;
                }
            }
        }
    }

    // --- Lateral: each cell pushes 1 unit to a strictly-lower +x / +z neighbor
    //     (same y) with room, equalizing surfaces over successive steps. Each
    //     unordered pair is visited once (owned by its lower-coordinate cell),
    //     and every transfer moves exactly 1 unit, so volume is preserved. ---
    for z in 0..out.nz {
        for y in 0..out.ny {
            for x in 0..out.nx {
                let here = out.get(x, y, z);
                if here == 0 {
                    continue;
                }
                // +x neighbor.
                if x + 1 < out.nx && !is_solid(x + 1, y, z) {
                    let n = out.get(x + 1, y, z);
                    if here > n + 1 && n < MAX_WATER {
                        out.set(x, y, z, out.get(x, y, z) - 1);
                        out.set(x + 1, y, z, n + 1);
                    }
                }
                let here = out.get(x, y, z);
                if here == 0 {
                    continue;
                }
                // +z neighbor.
                if z + 1 < out.nz && !is_solid(x, y, z + 1) {
                    let n = out.get(x, y, z + 1);
                    if here > n + 1 && n < MAX_WATER {
                        out.set(x, y, z, out.get(x, y, z) - 1);
                        out.set(x, y, z + 1, n + 1);
                    }
                }
            }
        }
    }
    out
}

// ── Buoyancy ────────────────────────────────────────────────────────────────

/// Manual `Fixed` division for the few body-scale ratios here (values are small
/// — a body is metres tall — so `a << 16` cannot overflow i64).
#[inline]
fn fixed_div(a: Fixed, b: Fixed) -> Fixed {
    if b.0 == 0 {
        Fixed::ZERO
    } else {
        Fixed((a.0 << 16) / b.0)
    }
}

/// Fraction of a body's height that is below the water surface, in `[0,1]`.
/// `body_bottom`/`body_top` are world-Y of the body's AABB; `surface` is the
/// water surface Y.
pub fn submerged_fraction(body_bottom: Fixed, body_top: Fixed, surface: Fixed) -> Fixed {
    if surface <= body_bottom {
        return Fixed::ZERO; // entirely above water
    }
    if surface >= body_top {
        return Fixed::ONE; // entirely submerged
    }
    let height = body_top - body_bottom;
    if height.0 <= 0 {
        return Fixed::ONE;
    }
    fixed_div(surface - body_bottom, height).clamp(Fixed::ZERO, Fixed::ONE)
}

/// Net vertical acceleration on a submerged body, replacing plain gravity while
/// it's in fluid. `density_ratio` = fluid_density / body_density (so a body
/// half as dense as water has ratio 2.0). `gravity` is the per-tick gravity
/// acceleration (negative = downward in this engine).
///
/// `accel = gravity · (1 − density_ratio · submerged)`:
///   - ratio > 1 (lighter than water), fully submerged → net points UP (rises),
///   - ratio < 1 (denser) → net points DOWN (sinks),
///   - ratio == 1 → net zero (neutrally buoyant, hovers).
pub fn buoyancy_accel(submerged: Fixed, density_ratio: Fixed, gravity: Fixed) -> Fixed {
    gravity * (Fixed::ONE - density_ratio * submerged)
}

/// Convenience: density ratio from integer densities (fluid over body).
pub fn density_ratio(fluid_density: i64, body_density: i64) -> Fixed {
    Fixed::from_frac(fluid_density, body_density.max(1))
}

/// Velocity after one tick of fluid drag: `vel − vel·drag`. `drag` in `[0,1]`.
pub fn fluid_drag(vel: Fixed, drag: Fixed) -> Fixed {
    vel - vel * drag.clamp(Fixed::ZERO, Fixed::ONE)
}

// ── Look data (tint / dirt) ───────────────────────────────────────────────

/// Water tint the surface shader applies: a clear shallow blue that deepens and
/// muddies with `depth_blocks` (how far light travels through water) and
/// `turbidity` (suspended dirt, 0 clear .. 1 muddy). Returns linear RGB.
/// Deeper → darker and bluer/greener; dirtier → browner and dimmer.
pub fn water_color(depth_blocks: f32, turbidity: f32) -> [f32; 3] {
    let d = (depth_blocks.max(0.0) / 24.0).clamp(0.0, 1.0); // 0 surface .. 1 deep
    let t = turbidity.clamp(0.0, 1.0);
    // Clear water: bright cyan at the surface → deep teal with depth.
    let shallow = [0.30_f32, 0.62, 0.75];
    let deep = [0.02_f32, 0.18, 0.28];
    let mut c = [
        shallow[0] + (deep[0] - shallow[0]) * d,
        shallow[1] + (deep[1] - shallow[1]) * d,
        shallow[2] + (deep[2] - shallow[2]) * d,
    ];
    // Dirt pulls toward a murky brown and dims overall.
    let mud = [0.34_f32, 0.27, 0.16];
    for i in 0..3 {
        c[i] = (c[i] + (mud[i] - c[i]) * t) * (1.0 - 0.25 * t);
    }
    c
}

// ── Surface, flow, and foam — the bridge to the water shader ──────────────────
//
// The shader can't see the voxel field; it gets these derived, sub-cell-smooth
// quantities. `surface_height` gives a continuous waterline to build the mesh /
// raymarch against; `flow_field` gives a horizontal velocity per column so the
// shader can scroll its normal map *along the current* and orient foam streaks;
// `surface_normal` perturbs the lighting; `foam` marks whitewater at rapids,
// spill edges, and shores. All f32, derived read-only from a settled field —
// they don't feed back into the deterministic sim.

/// Horizontal flow velocity per water column. `vx`/`vz` are downhill-of-surface
/// gradients (water runs from the higher surface toward the lower), in blocks
/// per tick-ish units; `[0,0]` on a flat pool.
#[derive(Debug, Clone, PartialEq)]
pub struct FlowField {
    pub nx: usize,
    pub nz: usize,
    /// Row-major (z * nx + x) of (vx, vz).
    pub vel: Vec<(f32, f32)>,
}

impl FlowField {
    pub fn get(&self, x: usize, z: usize) -> (f32, f32) {
        self.vel.get(z * self.nx + x).copied().unwrap_or((0.0, 0.0))
    }
    /// Flow speed (magnitude) at a column.
    pub fn speed(&self, x: usize, z: usize) -> f32 {
        let (vx, vz) = self.get(x, z);
        (vx * vx + vz * vz).sqrt()
    }
}

impl FluidField {
    /// Continuous water surface height (world-Y) for a column: the top filled
    /// cell plus its fractional fill, so a half-full top cell reads as `y+0.5`.
    /// `None` if the column is dry.
    pub fn surface_height(&self, x: usize, z: usize) -> Option<f32> {
        let top = self.surface_y(x, z)?;
        let frac = self.get(x, top, z) as f32 / MAX_WATER as f32;
        Some(top as f32 + frac)
    }
}

/// Speed scale applied to surface-gradient flow.
pub const FLOW_SPEED: f32 = 0.5;

/// Per-column horizontal flow from the surface gradient: water accelerates from
/// the higher surface toward the lower. Dry neighbours contribute no interior
/// flow (edges are handled by `foam`). Columns with no water read `[0,0]`.
pub fn flow_field(f: &FluidField) -> FlowField {
    let mut vel = vec![(0.0f32, 0.0f32); f.nx * f.nz];
    for z in 0..f.nz {
        for x in 0..f.nx {
            let Some(s) = f.surface_height(x, z) else { continue };
            // Central difference over wet neighbours; fall back to `s` (no slope)
            // where a neighbour is dry so edges don't generate spurious interior
            // flow.
            let sx_lo = if x > 0 { f.surface_height(x - 1, z).unwrap_or(s) } else { s };
            let sx_hi = if x + 1 < f.nx { f.surface_height(x + 1, z).unwrap_or(s) } else { s };
            let sz_lo = if z > 0 { f.surface_height(x, z - 1).unwrap_or(s) } else { s };
            let sz_hi = if z + 1 < f.nz { f.surface_height(x, z + 1).unwrap_or(s) } else { s };
            let vx = (sx_lo - sx_hi) * 0.5 * FLOW_SPEED;
            let vz = (sz_lo - sz_hi) * 0.5 * FLOW_SPEED;
            vel[z * f.nx + x] = (vx, vz);
        }
    }
    FlowField { nx: f.nx, nz: f.nz, vel }
}

/// Surface normal (unit) at a water column from the surface-height gradient.
/// A flat pool returns straight up `[0,1,0]`; a sloped surface tilts toward the
/// downhill direction. `[0,1,0]` if the column is dry.
pub fn surface_normal(f: &FluidField, x: usize, z: usize) -> [f32; 3] {
    let Some(s) = f.surface_height(x, z) else { return [0.0, 1.0, 0.0] };
    let sx_lo = if x > 0 { f.surface_height(x - 1, z).unwrap_or(s) } else { s };
    let sx_hi = if x + 1 < f.nx { f.surface_height(x + 1, z).unwrap_or(s) } else { s };
    let sz_lo = if z > 0 { f.surface_height(x, z - 1).unwrap_or(s) } else { s };
    let sz_hi = if z + 1 < f.nz { f.surface_height(x, z + 1).unwrap_or(s) } else { s };
    // Gradient of height; normal = (-dh/dx, 1, -dh/dz) normalized.
    let dhx = (sx_hi - sx_lo) * 0.5;
    let dhz = (sz_hi - sz_lo) * 0.5;
    let n = [-dhx, 1.0, -dhz];
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    [n[0] / len, n[1] / len, n[2] / len]
}

/// Foam / whitewater intensity in `[0,1]` at a water column. Three sources add
/// up (then clamp): fast flow (rapids), a sharp surface drop to a neighbour
/// (a spill edge or waterfall lip), and shallow water lapping over solid
/// ground (shore). `flow` is the field from [`flow_field`].
pub fn foam(
    f: &FluidField,
    flow: &FlowField,
    x: usize,
    z: usize,
    is_solid: impl Fn(usize, usize, usize) -> bool,
) -> f32 {
    let Some(s) = f.surface_height(x, z) else { return 0.0 };
    let mut foam = 0.0f32;

    // 1. Rapids: fast horizontal flow froths.
    foam += (flow.speed(x, z) * 1.5).min(1.0);

    // 2. Spill edge / waterfall lip: a neighbour column whose surface is far
    //    below ours (or dry) means water is pouring off here.
    let neighbours = [
        (x.wrapping_sub(1), z, x > 0),
        (x + 1, z, x + 1 < f.nx),
        (x, z.wrapping_sub(1), z > 0),
        (x, z + 1, z + 1 < f.nz),
    ];
    let mut max_drop = 0.0f32;
    for (nx, nz, inb) in neighbours {
        if !inb {
            continue;
        }
        let ns = f.surface_height(nx, nz).unwrap_or(s - 2.0); // dry = a big drop
        max_drop = max_drop.max(s - ns);
    }
    foam += (max_drop * 0.6).clamp(0.0, 1.0);

    // 3. Shore: a thin sheet of water (≤ ~1 cell) directly over solid ground.
    if let Some(top) = f.surface_y(x, z) {
        let shallow = f.get(x, top, z) <= (MAX_WATER / 2);
        let over_solid = top == 0 || is_solid(x, top.saturating_sub(1), z);
        if shallow && over_solid {
            foam += 0.3;
        }
    }

    foam.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ff(nx: usize, ny: usize, nz: usize) -> FluidField {
        FluidField::new(nx, ny, nz)
    }

    #[test]
    fn surface_height_is_subcell_continuous() {
        let mut f = ff(1, 4, 1);
        f.set(0, 0, 0, MAX_WATER); // full bottom cell
        f.set(0, 1, 0, MAX_WATER / 2); // half-full cell on top
        // Surface = top filled cell (y=1) + half a cell.
        assert_eq!(f.surface_height(0, 0), Some(1.5));
        // Dry column → None.
        assert_eq!(ff(1, 4, 1).surface_height(0, 0), None);
    }

    #[test]
    fn flat_pool_has_no_flow_and_an_up_normal() {
        let mut f = ff(3, 2, 3);
        for x in 0..3 {
            for z in 0..3 {
                f.set(x, 0, z, MAX_WATER);
            }
        }
        let flow = flow_field(&f);
        assert!(flow.speed(1, 1) < 1e-6, "still pool, no flow");
        let n = surface_normal(&f, 1, 1);
        assert!(n[1] > 0.999, "flat surface normal points up: {n:?}");
    }

    #[test]
    fn flow_runs_downhill_along_the_surface_gradient() {
        // A staircase of surface heights along +x: deeper on the low-x side.
        let mut f = ff(3, 4, 1);
        f.set(0, 0, 0, MAX_WATER);
        f.set(0, 1, 0, MAX_WATER); // column 0 surface ~2.0
        f.set(1, 0, 0, MAX_WATER); // column 1 surface ~1.0
        f.set(2, 0, 0, MAX_WATER / 2); // column 2 surface ~0.5
        let flow = flow_field(&f);
        let (vx, _) = flow.get(1, 0);
        assert!(vx > 0.0, "water flows toward the lower (+x) side, vx={vx}");
        // The sloped surface tilts its normal off vertical.
        let n = surface_normal(&f, 1, 0);
        assert!(n[0] > 0.0, "normal leans downhill (+x): {n:?}");
    }

    #[test]
    fn foam_high_at_a_spill_edge_low_on_a_calm_lake() {
        // Deep calm interior of a big pool: little foam.
        let mut lake = ff(5, 3, 5);
        for x in 0..5 {
            for z in 0..5 {
                lake.set(x, 0, z, MAX_WATER);
                lake.set(x, 1, z, MAX_WATER);
            }
        }
        let lake_flow = flow_field(&lake);
        assert!(foam(&lake, &lake_flow, 2, 2, |_, _, _| false) < 0.3, "calm interior");

        // A column of deep water with a dry neighbour → a spill edge → foam.
        let mut edge = ff(2, 4, 1);
        edge.set(0, 0, 0, MAX_WATER);
        edge.set(0, 1, 0, MAX_WATER);
        edge.set(0, 2, 0, MAX_WATER); // tall column at x=0, x=1 dry
        let edge_flow = flow_field(&edge);
        assert!(foam(&edge, &edge_flow, 0, 0, |_, _, _| false) > 0.4, "spill edge froths");
    }

    #[test]
    fn gravity_settles_water_to_the_bottom() {
        // Water floating at the top of an open column should fall to the floor.
        let mut f = ff(1, 5, 1);
        f.set(0, 4, 0, 6);
        let before = f.total();
        let after = flow_step(&f, |_, _, _| false);
        assert_eq!(after.total(), before, "volume conserved");
        assert_eq!(after.get(0, 0, 0), 6, "water settled to the floor");
        assert_eq!(after.get(0, 4, 0), 0, "top is now empty");
        assert_eq!(after.surface_y(0, 0), Some(0));
    }

    #[test]
    fn water_stacks_past_a_full_cell() {
        // 10 units in a 1-wide column → 8 (full) at the bottom, 2 above.
        let mut f = ff(1, 4, 1);
        f.set(0, 3, 0, 10); // set() clamps to MAX (8) on store...
        // ...so inject via two cells to get 10 total.
        f.set(0, 3, 0, 8);
        f.set(0, 2, 0, 2);
        let after = flow_step(&f, |_, _, _| false);
        assert_eq!(after.total(), 10);
        assert_eq!(after.get(0, 0, 0), 8, "bottom fills first");
        assert_eq!(after.get(0, 1, 0), 2, "remainder stacks above");
    }

    #[test]
    fn gravity_skips_solid_cells() {
        // A solid shelf at y=2 holds water above it; water below settles to floor.
        let mut f = ff(1, 5, 1);
        f.set(0, 4, 0, 3); // above the shelf
        f.set(0, 1, 0, 2); // below the shelf
        let solid = |_: usize, y: usize, _: usize| y == 2;
        let after = flow_step(&f, solid);
        assert_eq!(after.total(), 5, "volume conserved across the shelf");
        assert_eq!(after.get(0, 2, 0), 0, "no water in the solid cell");
        assert_eq!(after.get(0, 3, 0), 3, "water rests on top of the shelf");
        assert_eq!(after.get(0, 0, 0), 2, "water below settles to the floor");
    }

    #[test]
    fn lateral_spread_equalizes_and_conserves() {
        // Two adjacent floor cells, all water in one. Over steps they level out.
        let mut f = ff(2, 1, 1);
        f.set(0, 0, 0, 6);
        let v0 = f.total();
        let mut cur = f;
        for _ in 0..16 {
            cur = flow_step(&cur, |_, _, _| false);
        }
        assert_eq!(cur.total(), v0, "lateral flow conserves volume");
        let (a, b) = (cur.get(0, 0, 0), cur.get(1, 0, 0));
        assert!((a as i32 - b as i32).abs() <= 1, "levels equalize: {a} vs {b}");
    }

    #[test]
    fn submerged_fraction_spans_zero_to_one() {
        let bottom = Fixed::from_int(10);
        let top = Fixed::from_int(12); // a 2-tall body
        // Surface below the body → dry.
        assert_eq!(submerged_fraction(bottom, top, Fixed::from_int(9)), Fixed::ZERO);
        // Surface above the body → fully under.
        assert_eq!(submerged_fraction(bottom, top, Fixed::from_int(13)), Fixed::ONE);
        // Surface at the midpoint → half submerged.
        let half = submerged_fraction(bottom, top, Fixed::from_int(11));
        assert!((half - Fixed::from_frac(1, 2)).abs() < Fixed::from_frac(1, 100),
            "half-submerged ~0.5, got raw {}", half.0);
    }

    #[test]
    fn light_body_floats_dense_sinks_neutral_hovers() {
        let g = Fixed::from_frac(-2, 10); // -0.2/tick, downward
        let full = Fixed::ONE;
        // Lighter than water (ratio 2.0) → upward accel (positive).
        let light = buoyancy_accel(full, density_ratio(1000, 500), g);
        assert!(light.0 > 0, "light body should rise: raw {}", light.0);
        // Denser than water (ratio 0.5) → downward accel (negative).
        let dense = buoyancy_accel(full, density_ratio(1000, 2000), g);
        assert!(dense.0 < 0, "dense body should sink: raw {}", dense.0);
        // Neutral (ratio 1.0) → ~zero.
        let neutral = buoyancy_accel(full, density_ratio(1000, 1000), g);
        assert_eq!(neutral, Fixed::ZERO, "neutral body hovers");
        // Barely submerged → close to plain gravity (little buoyancy yet).
        let barely = buoyancy_accel(Fixed::from_frac(1, 100), density_ratio(1000, 500), g);
        assert!(barely.0 < 0, "barely-wet light body still falls: raw {}", barely.0);
    }

    #[test]
    fn fluid_drag_damps_toward_zero() {
        let v = Fixed::from_int(10);
        let half = fluid_drag(v, Fixed::from_frac(1, 2));
        assert_eq!(half, Fixed::from_int(5), "half drag halves velocity");
        // Repeated drag converges toward zero.
        let mut cur = v;
        for _ in 0..50 {
            cur = fluid_drag(cur, Fixed::from_frac(1, 4));
        }
        assert!(cur.abs() < Fixed::from_frac(1, 100), "velocity decays: raw {}", cur.0);
    }

    #[test]
    fn water_color_deepens_and_muddies() {
        let surface_clear = water_color(0.0, 0.0);
        let deep_clear = water_color(40.0, 0.0);
        // Deeper is darker (lower luminance).
        let lum = |c: [f32; 3]| c[0] + c[1] + c[2];
        assert!(lum(deep_clear) < lum(surface_clear), "deep water is darker");
        // Dirty water is browner: red channel rises relative to blue vs clear.
        let muddy = water_color(4.0, 1.0);
        let clear = water_color(4.0, 0.0);
        assert!(muddy[0] / muddy[2] > clear[0] / clear[2], "dirt shifts toward brown");
        // All channels stay in range.
        for c in [surface_clear, deep_clear, muddy, clear] {
            for ch in c {
                assert!((0.0..=1.0).contains(&ch), "channel out of range: {ch}");
            }
        }
    }
}
