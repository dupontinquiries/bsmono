//! mob_visibility.rs — per-view culling + instance LOD selection.
//!
//! [`crate::mob_instances`] packs mob transforms into instance batches and
//! [`crate::viewport_layout`] tiles the framebuffer for splitscreen — but
//! nothing decides *which* mobs a given view can see, or at what detail. This
//! module is that decision, once per (view × instance): a cheap frustum cull
//! plus a projected-screen-size → LOD tier. Cull first, then pack only the
//! survivors; and with one [`View`] per splitscreen rect it's naturally
//! per-view. Pure integer/float math — no clocks, no RNG, no rendering types
//! (R1/R11/R21), fully deterministic, std-only.
//!
//! The frustum test bounds the real rectangular frustum with a *spherical cone*
//! through its corners, so it is deliberately **conservative**: it never culls
//! something the exact frustum would keep (it may keep a few just-off-corner
//! objects). Conservative is the correct direction for visibility — a wrongly
//! culled mob pops out of existence; a wrongly kept one merely costs a draw.

/// A camera for one view (one splitscreen player, or the whole screen).
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub eye: [f32; 3],
    /// Look direction; normalized internally, so it need not be unit length.
    pub forward: [f32; 3],
    /// Vertical field of view, radians.
    pub fov_y: f32,
    /// Width / height of the view's pixel rect.
    pub aspect: f32,
    pub near: f32,
    pub far: f32,
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn norm(v: [f32; 3]) -> [f32; 3] {
    let l = dot(v, v).sqrt();
    if l < 1e-9 { [0.0, 0.0, 1.0] } else { [v[0] / l, v[1] / l, v[2] / l] }
}

impl View {
    /// Half-angle (radians) of the spherical cone that bounds the rectangular
    /// frustum — the corner direction relative to `forward`. Used by [`visible`].
    fn corner_half_angle(&self) -> f32 {
        let t = (self.fov_y * 0.5).tan();
        // corner tangent = sqrt(tan_v² + tan_h²), tan_h = aspect · tan_v
        (t * (1.0 + self.aspect * self.aspect).sqrt()).atan()
    }
}

/// Is a bounding sphere (`center`, `radius`) potentially visible in `view`?
/// Conservative spherical-cone + near/far depth test.
pub fn visible(view: &View, center: [f32; 3], radius: f32) -> bool {
    let f = norm(view.forward);
    let v = [center[0] - view.eye[0], center[1] - view.eye[1], center[2] - view.eye[2]];
    let depth = dot(v, f);
    // depth (near/far) slab, with the sphere's radius as slack
    if depth > view.far + radius {
        return false;
    }
    if depth < view.near - radius {
        return false;
    }
    let len = dot(v, v).sqrt();
    if len <= radius {
        return true; // eye inside the sphere
    }
    // Angle from the view axis to the sphere centre, minus the sphere's angular
    // radius, must be within the bounding cone.
    let ang = (depth / len).clamp(-1.0, 1.0).acos();
    let sphere_ang = (radius / len).clamp(0.0, 1.0).asin();
    ang - sphere_ang <= view.corner_half_angle()
}

/// Fraction of the viewport *height* the object's diameter spans at its depth —
/// the size a LOD decision keys on. `~1.0` fills the screen; `0.02` is a speck.
/// Depth is clamped to `near` so a point at the eye doesn't blow up.
pub fn projected_size(view: &View, center: [f32; 3], radius: f32) -> f32 {
    let f = norm(view.forward);
    let v = [center[0] - view.eye[0], center[1] - view.eye[1], center[2] - view.eye[2]];
    let depth = dot(v, f).max(view.near.max(1e-3));
    let half_h = (view.fov_y * 0.5).tan(); // half viewport height per unit depth
    // diameter / (viewport height at this depth) = 2r / (2·depth·half_h)
    radius / (depth * half_h)
}

/// Map a projected screen size to a LOD tier: `0` = finest (largest on screen),
/// increasing = coarser. `thresholds` are the size cutoffs in **descending**
/// order; a size at or above `thresholds[i]` yields tier `i`. With `k`
/// thresholds the coarsest tier is `k`. E.g. `[0.25, 0.10, 0.04]`:
/// ≥0.25→0, ≥0.10→1, ≥0.04→2, else 3.
pub fn lod_for(size: f32, thresholds: &[f32]) -> u8 {
    let mut tier = 0u8;
    for &t in thresholds {
        if size >= t {
            return tier;
        }
        tier += 1;
    }
    tier
}

/// Per-view cull **and** LOD in one call: `None` if the sphere is culled, else
/// `Some(lod_tier)` from its projected size. This is the decision the packer
/// wants per instance.
pub fn classify(view: &View, center: [f32; 3], radius: f32, lod_thresholds: &[f32]) -> Option<u8> {
    if !visible(view, center, radius) {
        return None;
    }
    Some(lod_for(projected_size(view, center, radius), lod_thresholds))
}

/// Cull a batch of bounding spheres against `view`, writing `(index, lod_tier)`
/// for each survivor into `out` (cleared first). `out` is reused across frames
/// so the per-view pass is allocation-free after warmup.
pub fn cull_batch(
    view: &View,
    spheres: &[([f32; 3], f32)],
    lod_thresholds: &[f32],
    out: &mut Vec<(usize, u8)>,
) {
    out.clear();
    for (i, &(center, radius)) in spheres.iter().enumerate() {
        if let Some(lod) = classify(view, center, radius, lod_thresholds) {
            out.push((i, lod));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> View {
        View {
            eye: [0.0, 0.0, 0.0],
            forward: [0.0, 0.0, 1.0], // looking down +Z
            fov_y: 60.0_f32.to_radians(),
            aspect: 16.0 / 9.0,
            near: 0.1,
            far: 100.0,
        }
    }

    #[test]
    fn dead_ahead_is_visible() {
        assert!(visible(&view(), [0.0, 0.0, 20.0], 0.5));
    }

    #[test]
    fn behind_the_camera_is_culled() {
        assert!(!visible(&view(), [0.0, 0.0, -5.0], 0.5));
    }

    #[test]
    fn beyond_far_is_culled() {
        assert!(!visible(&view(), [0.0, 0.0, 200.0], 0.5));
        // ...but just inside far (with radius slack) is kept
        assert!(visible(&view(), [0.0, 0.0, 100.5], 1.0));
    }

    #[test]
    fn far_to_the_side_is_culled_but_inside_the_cone_is_kept() {
        let v = view();
        let h = v.corner_half_angle();
        let depth = 20.0_f32;
        // inside: lateral offset at 0.8·H
        let inside_x = depth * (h * 0.8).tan();
        assert!(visible(&v, [inside_x, 0.0, depth], 0.1), "just inside the cone should be visible");
        // outside: lateral offset well past H
        let outside_x = depth * (h * 1.5).tan();
        assert!(!visible(&v, [outside_x, 0.0, depth], 0.1), "well outside the cone should be culled");
    }

    #[test]
    fn eye_inside_sphere_is_visible() {
        assert!(visible(&view(), [0.0, 0.0, 0.0], 2.0));
    }

    #[test]
    fn conservative_never_culls_an_on_axis_object_at_any_depth() {
        let v = view();
        for &d in &[0.2_f32, 1.0, 10.0, 50.0, 99.0] {
            assert!(visible(&v, [0.0, 0.0, d], 0.3), "on-axis at depth {d} must be visible");
        }
    }

    #[test]
    fn projected_size_shrinks_with_depth_and_grows_with_radius() {
        let v = view();
        let near = projected_size(&v, [0.0, 0.0, 5.0], 1.0);
        let far = projected_size(&v, [0.0, 0.0, 50.0], 1.0);
        assert!(near > far, "closer ⇒ larger on screen: {near} vs {far}");
        let big = projected_size(&v, [0.0, 0.0, 20.0], 2.0);
        let small = projected_size(&v, [0.0, 0.0, 20.0], 1.0);
        assert!(big > small, "bigger radius ⇒ larger on screen");
    }

    #[test]
    fn lod_tiers_are_monotonic_in_distance() {
        let v = view();
        let th = [0.25_f32, 0.10, 0.04];
        let mut prev = 0u8;
        for &d in &[2.0_f32, 8.0, 20.0, 60.0, 100.0] {
            let lod = classify(&v, [0.0, 0.0, d], 1.0, &th).expect("on-axis visible");
            assert!(lod >= prev, "LOD tier must not drop as distance rises: {prev} -> {lod} at {d}");
            prev = lod;
        }
    }

    #[test]
    fn a_huge_near_object_is_finest_and_a_speck_is_coarsest() {
        let v = view();
        let th = [0.25_f32, 0.10, 0.04];
        assert_eq!(classify(&v, [0.0, 0.0, 2.0], 2.0, &th), Some(0), "big & near ⇒ LOD 0");
        assert_eq!(classify(&v, [0.0, 0.0, 100.0], 0.2, &th), Some(3), "tiny & far ⇒ coarsest");
    }

    #[test]
    fn lod_for_boundaries() {
        let th = [0.25_f32, 0.10, 0.04];
        assert_eq!(lod_for(0.30, &th), 0);
        assert_eq!(lod_for(0.25, &th), 0); // at threshold ⇒ that tier
        assert_eq!(lod_for(0.10, &th), 1);
        assert_eq!(lod_for(0.05, &th), 2);
        assert_eq!(lod_for(0.01, &th), 3);
        assert_eq!(lod_for(0.5, &[]), 0, "no thresholds ⇒ single tier 0");
    }

    #[test]
    fn cull_batch_keeps_only_the_visible_and_is_deterministic() {
        let v = view();
        let th = [0.2_f32, 0.05];
        let spheres = [
            ([0.0, 0.0, 10.0], 1.0),   // 0: visible
            ([0.0, 0.0, -3.0], 1.0),   // 1: behind → culled
            ([0.0, 0.0, 500.0], 1.0),  // 2: beyond far → culled
            ([2.0, 0.0, 30.0], 0.5),   // 3: visible, small
        ];
        let mut a = Vec::new();
        let mut b = Vec::new();
        cull_batch(&v, &spheres, &th, &mut a);
        cull_batch(&v, &spheres, &th, &mut b);
        assert_eq!(a, b, "deterministic");
        let idxs: Vec<usize> = a.iter().map(|&(i, _)| i).collect();
        assert_eq!(idxs, vec![0, 3], "only the two in-frustum spheres survive, in order");
    }
}
