//! fog_blend.rs — pure exp2 distance-fog factor.
//!
//! GL_EXP2-style fog: `1 - 2^(-(density·dist)²)`, the fraction to blend toward
//! the fog colour (0 = no fog at the camera, → 1 far away). Pairs with the
//! shader fog roadmap. Pure, deterministic, panic-free (R1/R11/R21).

/// Fog amount at `dist` for `density`, in `[0,1]`. `0` at `dist == 0`, rising
/// monotonically toward `1`. Negative inputs are clamped to `0`.
pub fn fog(dist: f32, density: f32) -> f32 {
    let z = density.max(0.0) * dist.max(0.0);
    let f = 1.0 - (-(z * z)).exp2(); // 1 - 2^(-(z²))
    f.max(0.0).min(1.0)
}

// ── r334-fog ────────────────────────────────────────────────────────────────
// Distance fog with an ambient floor, a height×distance combiner, and the
// froxel↔march SEAM blend. The seam blend is the key parity piece: the froxel
// volume only covers a finite range (`render::froxel` = [0.5 m, 200 m]); the
// per-fragment march handles closer/farther. If the two don't agree at the
// boundary you get a hard-edged fog "step" — a fixed-position box where the
// volume ends (notably the near seam, right where a 3rd-person avatar sits in
// screen space). `froxel_seam_blend` crossfades across a band so the seam is
// continuous, and `distance_fog`'s floor keeps a near/unfilled region from
// reading as a hard clear box. Pure, finite, panic-free.
//
// Scalars in / scalar out (no FogParams import) so this file still gates
// standalone with `rustc --test`; the bin reads `height_fog::FogParams` fields
// and passes them here. On the r335 base, swap `smoothstep01` for `mathx`.

/// Smoothstep in `[0,1]` (Hermite). `edge0 == edge1` degenerates to a step.
/// Local copy because `mathx` (r332) isn't on this base — coordinator swaps it.
fn smoothstep01(edge0: f32, edge1: f32, x: f32) -> f32 {
    if !(edge1 > edge0) {
        return if x < edge0 { 0.0 } else { 1.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Distance fog lifted to an ambient `floor`: `fog(dist,density)` but never
/// below `floor`. Keeps a near-camera region from being a hard clear hole.
pub fn distance_fog(dist: f32, density: f32, floor: f32) -> f32 {
    let f = fog(dist, density);
    f.max(floor.clamp(0.0, 1.0)).min(1.0)
}

/// Combine a height factor and a distance factor (both `[0,1]`) into a final
/// fog amount in `[floor,1]`. Height modulates how much medium the ray crosses;
/// distance is the path-length term. Monotonic non-decreasing in *both* inputs,
/// so more height OR more distance never reduces fog.
pub fn combine_height_distance(height01: f32, distance01: f32, floor: f32) -> f32 {
    let h = height01.clamp(0.0, 1.0);
    let d = distance01.clamp(0.0, 1.0);
    (h * d).max(floor.clamp(0.0, 1.0)).min(1.0)
}

/// Weight of the froxel contribution at `view_dist`, in `[0,1]`: ~1 well inside
/// `[near_m, far_m]`, smoothly → 0 within `band_m` of each edge (and outside).
/// This is what makes the froxel↔march transition continuous.
pub fn froxel_blend_factor(view_dist: f32, near_m: f32, far_m: f32, band_m: f32) -> f32 {
    let near = near_m.max(0.0);
    let far = far_m.max(near);
    let band = band_m.max(0.0);
    if band == 0.0 {
        return if view_dist >= near && view_dist <= far { 1.0 } else { 0.0 };
    }
    // Ramp up over [near, near+band], ramp down over [far-band, far].
    let rise = smoothstep01(near, near + band, view_dist);
    let fall = 1.0 - smoothstep01(far - band, far, view_dist);
    (rise * fall).clamp(0.0, 1.0)
}

/// Blend the per-fragment `march_fog` with the `froxel_fog` sample so the two
/// agree across the froxel volume boundary — no hard fog step at the seam.
/// Equals `march_fog` outside the froxel range, `froxel_fog` well inside, and a
/// smooth crossfade across `band_m`. Result clamped to `[0,1]`.
pub fn froxel_seam_blend(
    march_fog: f32,
    froxel_fog: f32,
    view_dist: f32,
    near_m: f32,
    far_m: f32,
    band_m: f32,
) -> f32 {
    let m = march_fog.clamp(0.0, 1.0);
    let v = froxel_fog.clamp(0.0, 1.0);
    let w = froxel_blend_factor(view_dist, near_m, far_m, band_m);
    (m + (v - m) * w).clamp(0.0, 1.0)
}

#[cfg(test)]
mod r334_fog_tests {
    use super::*;

    #[test]
    fn distance_fog_floor_holds_near_camera() {
        // At the camera the bare curve is ~0; the floor keeps a min ambient.
        assert!(distance_fog(0.0, 0.02, 0.0) < 1e-6);
        assert!((distance_fog(0.0, 0.02, 0.15) - 0.15).abs() < 1e-6);
        // Floor never makes it exceed 1 or drop below the bare curve far out.
        assert!(distance_fog(1e6, 0.05, 0.1) <= 1.0);
    }

    #[test]
    fn combine_is_monotonic_in_both_axes() {
        assert!(combine_height_distance(0.8, 0.6, 0.0) >= combine_height_distance(0.4, 0.6, 0.0));
        assert!(combine_height_distance(0.6, 0.8, 0.0) >= combine_height_distance(0.6, 0.4, 0.0));
        // Floor lifts the near (distance→0) corner so it isn't a clear hole.
        assert!((combine_height_distance(1.0, 0.0, 0.12) - 0.12).abs() < 1e-6);
        // Bounded.
        for &(h, d) in &[(0.0, 0.0), (1.0, 1.0), (0.3, 0.9), (2.0, -1.0)] {
            let f = combine_height_distance(h, d, 0.05);
            assert!((0.0..=1.0).contains(&f));
        }
    }

    #[test]
    fn froxel_weight_is_one_inside_zero_outside() {
        let (near, far, band) = (0.5f32, 200.0f32, 4.0f32);
        assert_eq!(froxel_blend_factor(-1.0, near, far, band), 0.0); // behind near
        assert!(froxel_blend_factor(100.0, near, far, band) > 0.999); // mid-range
        assert_eq!(froxel_blend_factor(1e6, near, far, band), 0.0); // beyond far
    }

    #[test]
    fn seam_blend_is_continuous_within_tolerance() {
        // March and froxel disagree by a lot; the blend must not jump hard as
        // view_dist sweeps the near seam — the bonus "absent fog box" class.
        let (near, far, band) = (0.5f32, 200.0f32, 4.0f32);
        let march = 0.05f32; // near region: thin march fog
        let froxel = 0.85f32; // froxel says thick
        let mut prev = froxel_seam_blend(march, froxel, 0.0, near, far, band);
        let mut x = 0.0f32;
        while x <= 12.0 {
            let f = froxel_seam_blend(march, froxel, x, near, far, band);
            assert!((f - prev).abs() < 0.05, "seam jumped at dist={x}: {prev}->{f}");
            assert!((0.0..=1.0).contains(&f));
            prev = f;
            x += 0.1;
        }
        // Endpoints: equals march outside, froxel well inside.
        assert!((froxel_seam_blend(march, froxel, 0.0, near, far, band) - march).abs() < 1e-6);
        assert!((froxel_seam_blend(march, froxel, 100.0, near, far, band) - froxel).abs() < 1e-3);
    }

    #[test]
    fn seam_blend_far_edge_also_continuous() {
        let (near, far, band) = (0.5f32, 200.0f32, 4.0f32);
        let (march, froxel) = (0.9f32, 0.3f32);
        let a = froxel_seam_blend(march, froxel, 199.9, near, far, band);
        let b = froxel_seam_blend(march, froxel, 205.0, near, far, band);
        assert!((b - march).abs() < 1e-6, "must be march beyond far");
        assert!((a - b).abs() < 0.6, "far seam should not be a hard cliff");
    }

    #[test]
    fn deterministic() {
        assert_eq!(
            froxel_seam_blend(0.1, 0.9, 3.3, 0.5, 200.0, 4.0).to_bits(),
            froxel_seam_blend(0.1, 0.9, 3.3, 0.5, 200.0, 4.0).to_bits()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dist_zero_is_clear() {
        assert!(fog(0.0, 0.1) < 1e-6);
    }

    #[test]
    fn far_is_full_fog() {
        assert!((fog(1e6, 0.05) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn monotone_in_distance() {
        let mut prev = -1.0f32;
        let mut d = 0.0f32;
        while d <= 500.0 {
            let f = fog(d, 0.02);
            assert!(f >= prev - 1e-6, "fog dropped at {}", d);
            prev = f;
            d += 1.0;
        }
    }

    #[test]
    fn density_scales() {
        // more density => more fog at a fixed distance
        assert!(fog(50.0, 0.05) > fog(50.0, 0.01));
    }

    #[test]
    fn clamped_unit_range() {
        let mut seed = 0x1847_0001u32;
        let mut lcg = || { seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); (seed >> 8) as f32 / ((1u32 << 24) as f32) };
        for _ in 0..2000 {
            let f = fog(lcg() * 2000.0 - 100.0, lcg() * 0.5 - 0.1);
            assert!((0.0..=1.0).contains(&f), "fog oob {}", f);
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(fog(42.0, 0.03), fog(42.0, 0.03));
    }
}
