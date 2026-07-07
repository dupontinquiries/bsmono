//! wing_membrane.rs — pure dragon wing-skin (membrane) triangulation (T08-R27).
//!
//! The wing rig (articulation / ribbon geometry) gives a set of **finger-bone
//! tips** fanning out from a shared **wrist root**; the membrane is the filled
//! skin stretched across them. This builds that surface as a triangle fan from
//! the root across the finger-tip span, subdivided to a tunable detail so the
//! body/wing mesh can run at the higher base resolution (see [`WING_MESH_DETAIL`]).
//! Each vertex carries the membrane alpha so the consumer can honour the wing's
//! `wing_opacity` translucency.
//!
//! Pure, deterministic, panic-free (R1/R11/R21 style) — no GPU, no clock, no rng,
//! no `unwrap`/`expect` outside tests. Mirrors the pure-core shape of
//! `ribbon_mesh` and `wing_opacity`, so it can be unit-tested headlessly and
//! consumed by whatever view renders the dragon.
#![allow(dead_code)]

/// Base mesh resolution for the dragon body/wing surface (T08-R27, part 2).
///
/// Lifted to 128 from the earlier coarse fan. It's the number of membrane
/// panels emitted across the finger-tip span, i.e. the subdivision count the
/// triangle fan is resampled to — a single named knob so it stays tunable
/// without hunting through the mesh code. Higher = smoother skin, more verts
/// (`WING_MESH_DETAIL + 2` per wing membrane); no LOD logic, just the base.
pub const WING_MESH_DETAIL: usize = 128;

/// A triangulated wing membrane: a flat vertex/index mesh plus a per-vertex
/// alpha (membrane translucency, from `wing_opacity`).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct WingMembrane {
    /// Vertex positions: `[root, sample_0, sample_1, …, sample_detail]`.
    pub positions: Vec<[f32; 3]>,
    /// Triangle indices into `positions` (CCW fan from the root), `3 * panels`.
    pub indices: Vec<u32>,
    /// Per-vertex membrane alpha in `[0,1]`, parallel to `positions`.
    pub alpha: Vec<f32>,
}

impl WingMembrane {
    /// Number of triangles (membrane panels) in the surface.
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Sample the finger-tip polyline at parameter `t` in `[0,1]` (uniform over
/// segments). `tips.len()` must be `>= 2`.
fn sample_polyline(tips: &[[f32; 3]], t: f32) -> [f32; 3] {
    let seg_count = tips.len() - 1;
    let scaled = (t.clamp(0.0, 1.0) * seg_count as f32).clamp(0.0, seg_count as f32);
    let i = (scaled.floor() as usize).min(seg_count - 1);
    let local = scaled - i as f32;
    lerp(tips[i], tips[i + 1], local)
}

/// Triangulate the wing membrane.
///
/// * `finger_tips` — ordered bone-tip points along the wing's trailing edge
///   (leading finger first). Fewer than two tips can't span a surface ⇒ empty.
/// * `root` — the shared wrist/root point the fan radiates from.
/// * `detail` — number of membrane panels across the span (clamped `>= 1`); pass
///   [`WING_MESH_DETAIL`] for the base resolution.
/// * `alpha` — membrane opacity in `[0,1]` (clamped), typically
///   `wing_opacity(gene).alpha`. Written to every vertex.
///
/// The fan resamples the tip polyline to `detail + 1` evenly-parameterised
/// points and connects each adjacent pair back to `root`, yielding `detail`
/// triangles and `detail + 2` vertices.
pub fn membrane(finger_tips: &[[f32; 3]], root: [f32; 3], detail: usize, alpha: f32) -> WingMembrane {
    if finger_tips.len() < 2 {
        return WingMembrane::default();
    }
    let panels = detail.max(1);
    let a = clamp01(alpha);

    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(panels + 2);
    positions.push(root);
    for k in 0..=panels {
        let t = k as f32 / panels as f32;
        positions.push(sample_polyline(finger_tips, t));
    }

    let mut indices: Vec<u32> = Vec::with_capacity(panels * 3);
    for k in 0..panels {
        // root (0), sample k (1+k), sample k+1 (2+k)
        indices.push(0);
        indices.push((1 + k) as u32);
        indices.push((2 + k) as u32);
    }

    let alpha = vec![a; positions.len()];
    WingMembrane { positions, indices, alpha }
}

/// Build a wing-membrane fan that fills the footprint of a wing *box*.
///
/// The live entity geometry (`world::entity`) models a wing as an axis-aligned
/// box (center + half-extents). This converts such a box into a translucent
/// membrane fan: finger tips are sampled across the box's outer span edge and the
/// fan radiates from the inner (body-side) root. `span_axis` is the wingspan axis
/// (0=X, 1=Y, 2=Z; dragons span Z). Alpha comes from
/// [`crate::wing_opacity::wing_opacity`]. Pure — no renderer dependency; this is
/// the single entry point a creature renderer (or an alpha-capable entity vertex
/// path, once one exists) calls to draw wingskin instead of the opaque box.
pub fn membrane_from_box(
    center: [f32; 3],
    half: [f32; 3],
    span_axis: usize,
    detail: usize,
    opacity_gene: f32,
) -> WingMembrane {
    let a = span_axis.min(2);
    // Chord = the non-span axis with the larger half-extent (the wing's width).
    let chord = match a {
        0 => if half[1] >= half[2] { 1 } else { 2 },
        1 => if half[0] >= half[2] { 0 } else { 2 },
        _ => if half[0] >= half[1] { 0 } else { 1 },
    };
    let mut root = center;
    root[a] -= half[a]; // inner (body-side) edge
    let mut tip_lo = center;
    tip_lo[a] += half[a];
    tip_lo[chord] -= half[chord];
    let mut tip_hi = center;
    tip_hi[a] += half[a];
    tip_hi[chord] += half[chord];
    let mat = crate::wing_opacity::wing_opacity(opacity_gene);
    membrane(&[tip_lo, tip_hi], root, detail, mat.alpha)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (*seed >> 8) as f32 / ((1u32 << 24) as f32)
    }

    fn fan_tips(n: usize) -> Vec<[f32; 3]> {
        // A simple spread of tips along +x at increasing height.
        (0..n).map(|i| [1.0 + i as f32, i as f32 * 0.25, 0.0]).collect()
    }

    #[test]
    fn fewer_than_two_tips_is_empty() {
        assert!(membrane(&[], [0.0; 3], 128, 0.5).positions.is_empty());
        assert!(membrane(&[[1.0, 0.0, 0.0]], [0.0; 3], 128, 0.5).indices.is_empty());
    }

    #[test]
    fn counts_scale_with_detail() {
        for d in [1usize, 4, 32, 64, WING_MESH_DETAIL] {
            let m = membrane(&fan_tips(4), [0.0, 0.0, 0.0], d, 0.5);
            assert_eq!(m.positions.len(), d + 2, "verts = root + (detail+1)");
            assert_eq!(m.indices.len(), d * 3, "3 indices per panel");
            assert_eq!(m.triangle_count(), d);
            assert_eq!(m.alpha.len(), m.positions.len(), "one alpha per vertex");
        }
    }

    #[test]
    fn detail_128_is_the_base_resolution() {
        let m = membrane(&fan_tips(5), [0.0, 0.0, 0.0], WING_MESH_DETAIL, 0.7);
        assert_eq!(WING_MESH_DETAIL, 128);
        assert_eq!(m.triangle_count(), 128);
    }

    #[test]
    fn alpha_respects_wing_opacity_and_is_clamped() {
        // Mirrors what `wing_opacity(gene).alpha` would feed in.
        for (input, expect) in [(0.08f32, 0.08f32), (1.0, 1.0), (-1.0, 0.0), (2.0, 1.0)] {
            let m = membrane(&fan_tips(3), [0.0; 3], 8, input);
            assert!(m.alpha.iter().all(|&v| (v - expect).abs() < 1e-6), "alpha {input} -> {expect}");
            assert!(m.alpha.iter().all(|&v| (0.0..=1.0).contains(&v)));
        }
    }

    #[test]
    fn indices_are_in_range_and_fan_from_root() {
        let m = membrane(&fan_tips(4), [0.0, 0.0, 0.0], 16, 0.5);
        let vn = m.positions.len() as u32;
        for tri in m.indices.chunks_exact(3) {
            assert_eq!(tri[0], 0, "every triangle includes the root vertex");
            assert!(tri[1] < vn && tri[2] < vn, "index in range");
            assert_eq!(tri[2], tri[1] + 1, "adjacent samples");
        }
    }

    #[test]
    fn samples_stay_within_input_bounds_and_finite() {
        let tips = fan_tips(5);
        let m = membrane(&tips, [0.0, 0.0, 0.0], WING_MESH_DETAIL, 0.5);
        // bbox of tips ∪ root
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for p in tips.iter().chain(std::iter::once(&[0.0, 0.0, 0.0])) {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        for p in &m.positions {
            for k in 0..3 {
                assert!(p[k].is_finite());
                assert!(p[k] >= lo[k] - 1e-4 && p[k] <= hi[k] + 1e-4, "sample within tip bounds");
            }
        }
    }

    #[test]
    fn higher_detail_makes_more_triangles() {
        let t = fan_tips(4);
        assert!(membrane(&t, [0.0; 3], 128, 0.5).triangle_count()
            > membrane(&t, [0.0; 3], 32, 0.5).triangle_count());
    }

    #[test]
    fn deterministic() {
        let t = fan_tips(4);
        assert_eq!(membrane(&t, [0.1, 0.2, 0.3], 64, 0.42), membrane(&t, [0.1, 0.2, 0.3], 64, 0.42));
    }

    #[test]
    fn prop_no_nan_random() {
        let mut s = 0x1E3A_0001u32;
        for _ in 0..200 {
            let n = (lcg(&mut s) * 8.0) as usize;
            let tips: Vec<[f32; 3]> = (0..n)
                .map(|_| [lcg(&mut s) * 10.0 - 5.0, lcg(&mut s) * 10.0 - 5.0, lcg(&mut s) * 10.0 - 5.0])
                .collect();
            let m = membrane(&tips, [lcg(&mut s), lcg(&mut s), lcg(&mut s)], (lcg(&mut s) * 130.0) as usize, lcg(&mut s));
            for p in &m.positions {
                assert!(p.iter().all(|c| c.is_finite()));
            }
        }
    }

    #[test]
    fn membrane_from_box_fills_box_and_tracks_opacity() {
        // Wing box: wingspan along Z, chord along X.
        let center = [0.0, 0.0, 0.0];
        let half = [0.5, 0.1, 2.0];
        let m = membrane_from_box(center, half, 2, WING_MESH_DETAIL, 1.0);
        assert!(m.triangle_count() >= 1, "membrane should have panels");
        assert_eq!(m.positions.len(), m.alpha.len(), "alpha parallel to positions");
        // positions[0] is the root (inner -Z edge); tips lie on the +Z outer edge.
        assert!(m.positions[0][2] < -1.5, "root at inner -Z edge, got {:?}", m.positions[0]);
        assert!(m.positions.iter().any(|p| p[2] > 1.5), "tips at outer +Z edge");
        // Opaque gene (1.0) -> alpha well above the glassy baseline (0.08).
        assert!(m.alpha.iter().all(|&a| a > 0.5), "opaque wing should be mostly solid");
        // Glassy gene (0.0) -> faint.
        let g = membrane_from_box(center, half, 2, 8, 0.0);
        assert!(g.alpha.iter().all(|&a| a < 0.2), "glassy wing should be faint");
    }
}
