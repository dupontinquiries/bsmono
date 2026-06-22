//! Field brushes — the shared primitive behind elemental items.
//!
//! Per `docs/ELEMENTS.md`, an elemental item is not a scripted effect; it is
//! a *brush that perturbs a field*. A fire item injects heat into the
//! temperature field, a water item drains the moisture field, and the
//! world's response (evaporation, weather suppression, freezing) emerges from
//! how the fields are coupled — there is no type chart. This module is the
//! op every such item shares: add a signed, radially-falling-off delta to a
//! scalar field over a sphere of cells.
//!
//! It is deliberately **storage-agnostic**. It computes *what* delta goes
//! *where* (offsets + signed amounts), with no opinion on which field or how
//! it's stored. The interaction layer maps these writes onto the concrete
//! fields — temperature/humidity/pressure in [`crate::world::fields`], water
//! depth in [`crate::world::water_field`] — by implementing the small
//! [`ScalarField3`] / [`ScalarField2`] traits. This is the same "pure
//! functional unit, wired in later" pattern as
//! [`crate::world::pathing`] and [`crate::world::octree_raycast`].
//!
//! The cell set a brush touches is identical to
//! [`crate::world::area_mining::sphere_offsets`] at integer radius (it uses
//! the same `radius + 0.5` reach); a brush is "a sphere with falloff
//! weights." A negative `peak` drains rather than injects, so the same op
//! gives both halves of an element (flood/drain, heat/chill).

/// Radial falloff from the brush center (`t = 0`) to its edge (`t = 1`),
/// where `t` is distance normalized by the brush's reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Falloff {
    /// Full strength everywhere inside the reach, hard cutoff at the edge.
    Constant,
    /// Linear taper: `1 - t`.
    Linear,
    /// Smoothstep taper `3u² - 2u³` (`u = 1 - t`): a fuller core and a soft
    /// shoulder. The natural default for a "splash" of influence.
    Smooth,
}

impl Falloff {
    /// Weight in `[0, 1]` for normalized distance `t`. `t <= 0` → `1.0`
    /// (center); `t >= 1` → edge value (`0.0` for Linear/Smooth, `1.0` for
    /// Constant).
    pub fn weight(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Falloff::Constant => 1.0,
            Falloff::Linear => 1.0 - t,
            Falloff::Smooth => {
                let u = 1.0 - t;
                u * u * (3.0 - 2.0 * u)
            }
        }
    }
}

/// A radial perturbation of a scalar field: add `peak` at the center, fading
/// to ~0 at `radius` cells by `falloff`. `peak` is **signed** — positive
/// injects (flood, heat, humidify), negative drains (dry, chill, dehumidify).
#[derive(Debug, Clone, Copy)]
pub struct FieldBrush {
    /// Radius in cells. Fractional is allowed; the touched set at integer
    /// radius matches `area_mining::sphere_offsets`.
    pub radius: f32,
    /// Signed delta applied at the exact center.
    pub peak: f32,
    /// How the delta tapers from center to edge.
    pub falloff: Falloff,
}

impl FieldBrush {
    pub fn new(radius: f32, peak: f32, falloff: Falloff) -> Self {
        FieldBrush {
            radius,
            peak,
            falloff,
        }
    }

    /// Outer reach in cells: a cell is touched when its center is within this
    /// Euclidean distance of the brush center. `radius + 0.5`, matching
    /// [`crate::world::area_mining::sphere_offsets`].
    pub fn reach(&self) -> f32 {
        self.radius.max(0.0) + 0.5
    }

    /// Signed delta at Euclidean distance `dist` (in cells) from the center.
    /// Zero beyond the reach.
    pub fn delta_at(&self, dist: f32) -> f32 {
        let r = self.reach();
        if dist > r {
            return 0.0;
        }
        self.peak * self.falloff.weight(dist / r)
    }
}

/// A 3D brush write: offset `(dx, dy, dz)` from the brush center and the
/// signed delta to add there.
pub type BrushWrite3 = (i32, i32, i32, f32);

/// A 2D (per-column) brush write: offset `(dx, dz)` and the signed delta.
pub type BrushWrite2 = (i32, i32, f32);

/// The 3D cells a brush touches, each paired with its signed delta. The
/// offsets are relative to the brush center. The cell positions match
/// `area_mining::sphere_offsets(radius)` at integer radius; here each also
/// carries its falloff-weighted delta.
pub fn brush_cells(brush: &FieldBrush) -> Vec<BrushWrite3> {
    let r = brush.reach();
    let r_sq = r * r;
    let lim = r.ceil() as i32;
    let mut out = Vec::new();
    for dy in -lim..=lim {
        for dx in -lim..=lim {
            for dz in -lim..=lim {
                let d_sq = (dx * dx + dy * dy + dz * dz) as f32;
                if d_sq <= r_sq {
                    out.push((dx, dy, dz, brush.delta_at(d_sq.sqrt())));
                }
            }
        }
    }
    out
}

/// The 2D column version, for per-column fields like
/// [`crate::world::water_field`] (depth indexed by `x, z`). Offsets are
/// relative to the brush center column.
pub fn brush_columns(brush: &FieldBrush) -> Vec<BrushWrite2> {
    let r = brush.reach();
    let r_sq = r * r;
    let lim = r.ceil() as i32;
    let mut out = Vec::new();
    for dx in -lim..=lim {
        for dz in -lim..=lim {
            let d_sq = (dx * dx + dz * dz) as f32;
            if d_sq <= r_sq {
                out.push((dx, dz, brush.delta_at(d_sq.sqrt())));
            }
        }
    }
    out
}

/// Add `delta` to `base`, clamped to `[lo, hi]`. Most fields are bounded
/// (humidity/cloud cover in `[0, 1]`, water depth `>= 0`); this keeps a brush
/// from pushing a field out of its valid range. `lo` must not exceed `hi`.
pub fn accumulate(base: f32, delta: f32, lo: f32, hi: f32) -> f32 {
    (base + delta).clamp(lo, hi)
}

/// A 3D scalar field a brush can write into. The interaction layer implements
/// this for the concrete fields (e.g. temperature/humidity in
/// `world::fields`); the brush itself stays storage-agnostic. Implementors
/// typically clamp inside `add_delta` via [`accumulate`].
pub trait ScalarField3 {
    fn add_delta(&mut self, x: i32, y: i32, z: i32, delta: f32);
}

/// A per-column (2D) scalar field a brush can write into, e.g. water depth.
pub trait ScalarField2 {
    fn add_delta(&mut self, x: i32, z: i32, delta: f32);
}

/// Apply `brush` centered at world cell `(cx, cy, cz)` to a 3D field.
pub fn apply_brush_3d<F: ScalarField3>(
    field: &mut F,
    cx: i32,
    cy: i32,
    cz: i32,
    brush: &FieldBrush,
) {
    for (dx, dy, dz, delta) in brush_cells(brush) {
        field.add_delta(cx + dx, cy + dy, cz + dz, delta);
    }
}

/// Apply `brush` centered at column `(cx, cz)` to a per-column field.
pub fn apply_brush_2d<F: ScalarField2>(field: &mut F, cx: i32, cz: i32, brush: &FieldBrush) {
    for (dx, dz, delta) in brush_columns(brush) {
        field.add_delta(cx + dx, cz + dz, delta);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::area_mining::sphere_offsets;
    use std::collections::HashMap;

    #[test]
    fn weight_endpoints_and_monotonic() {
        for f in [Falloff::Constant, Falloff::Linear, Falloff::Smooth] {
            assert_eq!(f.weight(0.0), 1.0, "center is full strength");
        }
        assert_eq!(Falloff::Constant.weight(1.0), 1.0);
        assert_eq!(Falloff::Linear.weight(1.0), 0.0);
        assert_eq!(Falloff::Smooth.weight(1.0), 0.0);
        // Non-increasing as t grows, for the tapering falloffs.
        for f in [Falloff::Linear, Falloff::Smooth] {
            let mut prev = f.weight(0.0);
            for i in 1..=20 {
                let w = f.weight(i as f32 / 20.0);
                assert!(w <= prev + 1e-6, "{:?} should not rise with distance", f);
                assert!((0.0..=1.0).contains(&w));
                prev = w;
            }
        }
    }

    #[test]
    fn smooth_has_fuller_core_than_linear() {
        // Equal at the midpoint, fuller near the center.
        assert!((Falloff::Smooth.weight(0.5) - 0.5).abs() < 1e-6);
        assert!((Falloff::Linear.weight(0.5) - 0.5).abs() < 1e-6);
        assert!(Falloff::Smooth.weight(0.25) > Falloff::Linear.weight(0.25));
    }

    #[test]
    fn center_delta_equals_peak() {
        for f in [Falloff::Constant, Falloff::Linear, Falloff::Smooth] {
            let b = FieldBrush::new(2.0, 3.0, f);
            assert!((b.delta_at(0.0) - 3.0).abs() < 1e-6);
        }
    }

    #[test]
    fn delta_is_zero_beyond_reach() {
        let b = FieldBrush::new(2.0, 5.0, Falloff::Linear);
        assert_eq!(b.delta_at(b.reach() + 1.0), 0.0);
    }

    #[test]
    fn cell_counts_match_sphere_convention() {
        // Same reach as area_mining::sphere_offsets -> same counts.
        assert_eq!(brush_cells(&FieldBrush::new(0.0, 1.0, Falloff::Linear)).len(), 1);
        assert_eq!(brush_cells(&FieldBrush::new(1.0, 1.0, Falloff::Linear)).len(), 19);
        assert_eq!(brush_cells(&FieldBrush::new(2.0, 1.0, Falloff::Linear)).len(), 81);
        assert_eq!(brush_cells(&FieldBrush::new(3.0, 1.0, Falloff::Linear)).len(), 179);
    }

    #[test]
    fn cell_positions_equal_sphere_offsets() {
        // The touched set is exactly the sphere shape at integer radius.
        let brush_pos: Vec<(i32, i32, i32)> = brush_cells(&FieldBrush::new(2.0, 1.0, Falloff::Smooth))
            .into_iter()
            .map(|(x, y, z, _)| (x, y, z))
            .collect();
        let sphere: Vec<(i32, i32, i32)> = sphere_offsets(2);
        let mut a = brush_pos.clone();
        let mut b = sphere.clone();
        a.sort();
        b.sort();
        assert_eq!(a, b, "brush cells must match sphere_offsets at integer radius");
    }

    #[test]
    fn column_counts() {
        assert_eq!(brush_columns(&FieldBrush::new(0.0, 1.0, Falloff::Linear)).len(), 1);
        assert_eq!(brush_columns(&FieldBrush::new(1.0, 1.0, Falloff::Linear)).len(), 9);
        assert_eq!(brush_columns(&FieldBrush::new(2.0, 1.0, Falloff::Linear)).len(), 21);
    }

    #[test]
    fn constant_fills_uniformly() {
        let cells = brush_cells(&FieldBrush::new(2.0, 5.0, Falloff::Constant));
        assert_eq!(cells.len(), 81);
        assert!(cells.iter().all(|&(_, _, _, d)| (d - 5.0).abs() < 1e-6));
    }

    #[test]
    fn negative_peak_drains() {
        let cells = brush_cells(&FieldBrush::new(1.0, -2.0, Falloff::Linear));
        let center = cells.iter().find(|&&(x, y, z, _)| (x, y, z) == (0, 0, 0)).unwrap().3;
        assert!((center - (-2.0)).abs() < 1e-6, "center drains by full peak");
        assert!(cells.iter().all(|&(_, _, _, d)| d <= 1e-9), "every delta drains");
    }

    #[test]
    fn accumulate_clamps_into_range() {
        assert_eq!(accumulate(0.8, 0.5, 0.0, 1.0), 1.0); // clamps high
        assert_eq!(accumulate(0.3, -0.5, 0.0, 1.0), 0.0); // clamps low
        assert!((accumulate(0.5, 0.2, 0.0, 1.0) - 0.7).abs() < 1e-6); // in range
    }

    // A trivial grid for exercising the apply paths through the trait.
    #[derive(Default)]
    struct MapField3(HashMap<(i32, i32, i32), f32>);
    impl ScalarField3 for MapField3 {
        fn add_delta(&mut self, x: i32, y: i32, z: i32, delta: f32) {
            *self.0.entry((x, y, z)).or_insert(0.0) += delta;
        }
    }

    #[test]
    fn apply_brush_3d_writes_through_trait() {
        let mut field = MapField3::default();
        let brush = FieldBrush::new(2.0, 1.0, Falloff::Linear);
        apply_brush_3d(&mut field, 100, 64, -50, &brush);
        // One write per touched cell.
        assert_eq!(field.0.len(), 81);
        // The center cell got the full peak.
        assert!((field.0[&(100, 64, -50)] - 1.0).abs() < 1e-6);
        // A cell well outside the reach was never written.
        assert!(!field.0.contains_key(&(110, 64, -50)));
    }

    #[derive(Default)]
    struct MapField2(HashMap<(i32, i32), f32>);
    impl ScalarField2 for MapField2 {
        fn add_delta(&mut self, x: i32, z: i32, delta: f32) {
            *self.0.entry((x, z)).or_insert(0.0) += delta;
        }
    }

    #[test]
    fn apply_brush_2d_writes_through_trait() {
        let mut field = MapField2::default();
        // Drain (negative peak), as a water-dehumidify brush would.
        let brush = FieldBrush::new(1.0, -3.0, Falloff::Constant);
        apply_brush_2d(&mut field, 7, 7, &brush);
        assert_eq!(field.0.len(), 9);
        assert!((field.0[&(7, 7)] - (-3.0)).abs() < 1e-6);
        assert!(field.0.values().all(|&v| v <= 1e-9));
    }
}
