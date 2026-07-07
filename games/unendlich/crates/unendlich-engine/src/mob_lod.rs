//! mob_lod.rs — pure mob instance LOD selector.
//!
//! Picks a discrete level-of-detail from a mob's distance, the framebuffer
//! height, the mob's world radius and the vertical FOV (its apparent on-screen
//! size), and decides which body parts survive at that level. Composes with
//! `mob_instances` (the packed instances) and the part-LOD idea. Pure math,
//! deterministic, panic-free (R1/R11/R21). The actual mesh swap is the
//! coordinator's render pass.

/// Highest LOD index (coarsest). Levels run `0` (full detail) .. `MAX_LOD`
/// (billboard / lowest). 5 bands (0..=4).
pub const MAX_LOD: u8 = 4;

/// Apparent-diameter thresholds in pixels, descending. A mob whose on-screen
/// diameter is `>= THRESHOLDS_PX[k]` gets LOD `k`; below the last threshold it
/// gets `MAX_LOD`. (`THRESHOLDS_PX.len() == MAX_LOD as usize`.)
const THRESHOLDS_PX: [f32; 4] = [320.0, 120.0, 48.0, 16.0];

/// Apparent on-screen diameter, in pixels, of a sphere of `world_radius` at
/// `distance`, under vertical field of view `fov_y_rad` on a `screen_h`-pixel
/// tall framebuffer. Distance and FOV are floored to small positive values so
/// the result is always finite.
pub fn apparent_px(distance: f32, world_radius: f32, screen_h: u32, fov_y_rad: f32) -> f32 {
    let d = distance.max(1e-3);
    let fov = fov_y_rad.clamp(1e-3, core::f32::consts::PI - 1e-3);
    let r = world_radius.max(0.0);
    // projected half-angle ~ atan(r/d); pixels-per-radian = screen_h / fov.
    let half_angle = (r / d).atan();
    (2.0 * half_angle) * (screen_h as f32 / fov)
}

/// Select a LOD level from an apparent pixel diameter. Larger = more detail
/// (lower index). Monotone: smaller `px` never yields more detail.
pub fn lod_for_px(px: f32) -> u8 {
    let mut lod = 0u8;
    for &t in &THRESHOLDS_PX {
        if px < t {
            lod += 1;
        }
    }
    lod
}

/// Convenience: select a LOD straight from world parameters.
pub fn select_lod(distance: f32, world_radius: f32, screen_h: u32, fov_y_rad: f32) -> u8 {
    lod_for_px(apparent_px(distance, world_radius, screen_h, fov_y_rad))
}

/// Per-part keep mask for a given `lod`: a part of world radius `r` is kept while
/// `r >= base_part_radius * (lod + 1)`, so coarser levels drop the smallest
/// parts first. Keeping is monotone in detail: a finer LOD keeps a superset.
pub fn keep_parts(lod: u8, part_radii: &[f32], base_part_radius: f32) -> Vec<bool> {
    let base = base_part_radius.max(0.0);
    let threshold = base * (lod as f32 + 1.0);
    part_radii.iter().map(|&r| r.max(0.0) >= threshold).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOV: f32 = 1.0; // ~57 degrees

    #[test]
    fn prop_nearer_is_at_least_as_detailed() {
        // distance increasing -> LOD index non-decreasing (detail non-increasing).
        let mut prev = 0u8;
        let mut d = 0.5f32;
        while d <= 5000.0 {
            let lod = select_lod(d, 1.0, 1080, FOV);
            assert!(lod >= prev, "detail increased with distance at d={}", d);
            assert!(lod <= MAX_LOD, "lod out of range {}", lod);
            prev = lod;
            d *= 1.15;
        }
    }

    #[test]
    fn prop_lod_bounded() {
        for &px in &[0.0f32, 1.0, 17.0, 50.0, 200.0, 1000.0, 1e9] {
            assert!(lod_for_px(px) <= MAX_LOD);
        }
        assert_eq!(lod_for_px(1e9), 0, "huge mob -> full detail");
        assert_eq!(lod_for_px(0.0), MAX_LOD, "vanishing mob -> coarsest");
    }

    #[test]
    fn prop_keep_parts_monotone_in_detail() {
        // A finer LOD (lower index) keeps a superset of a coarser LOD's parts.
        let radii = [0.5f32, 0.2, 0.1, 0.05, 0.02, 0.4, 0.3];
        let base = 0.06;
        for lod in 0..MAX_LOD {
            let fine = keep_parts(lod, &radii, base);
            let coarse = keep_parts(lod + 1, &radii, base);
            for i in 0..radii.len() {
                assert!(fine[i] || !coarse[i], "coarse kept a part the fine LOD dropped (i={})", i);
            }
            let fine_count = fine.iter().filter(|&&k| k).count();
            let coarse_count = coarse.iter().filter(|&&k| k).count();
            assert!(coarse_count <= fine_count, "coarser kept more parts");
        }
    }

    #[test]
    fn prop_apparent_px_finite_and_monotone_in_distance() {
        let mut prev = f32::INFINITY;
        let mut d = 0.1f32;
        while d <= 10000.0 {
            let px = apparent_px(d, 1.0, 1080, FOV);
            assert!(px.is_finite() && px >= 0.0);
            assert!(px <= prev + 1e-3, "apparent size grew with distance at d={}", d);
            prev = px;
            d *= 1.2;
        }
        // degenerate inputs stay finite
        assert!(apparent_px(0.0, 0.0, 0, 0.0).is_finite());
        assert!(apparent_px(-5.0, -1.0, 1080, -1.0).is_finite());
    }

    #[test]
    fn prop_deterministic() {
        for &d in &[1.0f32, 10.0, 100.0, 1000.0] {
            assert_eq!(select_lod(d, 1.0, 1080, FOV), select_lod(d, 1.0, 1080, FOV));
        }
    }
}
