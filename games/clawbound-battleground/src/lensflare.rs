//! lensflare -- when and how strongly the sun flares the lens (post.wgsl
//! `lens_flare` draws it).
//!
//! The shader decides WHERE (ghosts strung from the sun through the frame's
//! centre at [`GHOSTS`] of the way, a starburst, an anamorphic streak, a
//! halo ring) and whether the sun is covered (the depth buffer round its
//! disc). This decides how much: [`strength`], from where the sun is on
//! screen and what the sky is doing, by the quality setting.

use crate::quality::Quality;

/// The ghosts' places along sun -> centre (1.0 = the frame's centre), as
/// post.wgsl's `ks` array has them.
pub const GHOSTS: [f32; 6] = [0.45, 0.72, 1.18, 1.55, 1.95, 2.4];

/// How much the flare shows at each quality: none on Low (it is a fill
/// cost), a little less on Medium.
pub fn quality_scale(q: Quality) -> f32 {
    match q {
        Quality::Low => 0.0,
        Quality::Medium => 0.6,
        Quality::High | Quality::Ultra => 1.0,
    }
}

/// The flare's strength: `sun_ndc` is the sun's position on screen in
/// normalised device coordinates (None when it is behind the camera). It
/// fades out as the sun leaves the frame, and with cloud, night and an
/// underwater view (the sea scatters it into a glow instead).
pub fn strength(sun_ndc: Option<[f32; 2]>, cloud: f32, night: f32, underwater: f32, q: Quality) -> f32 {
    let Some([x, y]) = sun_ndc else { return 0.0 };
    let off = (x.abs() - 1.0).max(0.0) + (y.abs() - 1.0).max(0.0);
    let in_frame = (1.0 - off * 2.5).clamp(0.0, 1.0);
    in_frame * (1.0 - cloud * 0.9).max(0.0) * (1.0 - night).max(0.0) * (1.0 - 0.7 * underwater.clamp(0.0, 1.0)) * quality_scale(q)
}

/// Where ghost `k` sits in uv for a sun at `sun_uv`.
pub fn ghost_at(sun_uv: [f32; 2], k: usize) -> [f32; 2] {
    let g = GHOSTS[k];
    [sun_uv[0] + (0.5 - sun_uv[0]) * g, sun_uv[1] + (0.5 - sun_uv[1]) * g]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flare_needs_a_visible_sun_a_clear_day_and_a_quality_that_pays_for_it() {
        let clear = strength(Some([0.3, 0.4]), 0.0, 0.0, 0.0, Quality::High);
        assert!((clear - 1.0).abs() < 1e-6);
        assert_eq!(strength(None, 0.0, 0.0, 0.0, Quality::High), 0.0, "behind the camera");
        assert_eq!(strength(Some([1.5, 0.0]), 0.0, 0.0, 0.0, Quality::High), 0.0, "well off the frame");
        let edge = strength(Some([1.1, 0.0]), 0.0, 0.0, 0.0, Quality::High);
        assert!(edge > 0.0 && edge < clear, "fading at the edge: {edge}");
        assert!(strength(Some([0.3, 0.4]), 0.8, 0.0, 0.0, Quality::High) < 0.35, "cloud");
        assert_eq!(strength(Some([0.3, 0.4]), 0.0, 1.0, 0.0, Quality::High), 0.0, "night");
        assert!(strength(Some([0.3, 0.4]), 0.0, 0.0, 1.0, Quality::High) < 0.35, "under the sea");
        assert_eq!(strength(Some([0.3, 0.4]), 0.0, 0.0, 0.0, Quality::Low), 0.0, "Low pays nothing");
        assert!(strength(Some([0.3, 0.4]), 0.0, 0.0, 0.0, Quality::Medium) < clear);
    }

    /// The ghosts run from the sun through the centre and out the other
    /// side, in order -- and the shader's table is this one.
    #[test]
    fn the_ghosts_string_through_the_centre() {
        let sun = [0.8, 0.2];
        let mut last = -1.0f32;
        for k in 0..GHOSTS.len() {
            let g = ghost_at(sun, k);
            // on the line: the cross product with the axis is zero
            let (ax, ay) = (0.5 - sun[0], 0.5 - sun[1]);
            let (gx, gy) = (g[0] - sun[0], g[1] - sun[1]);
            assert!((ax * gy - ay * gx).abs() < 1e-6);
            let t = (gx * ax + gy * ay) / (ax * ax + ay * ay);
            assert!(t > last);
            last = t;
        }
        assert!(last > 1.0, "some ghosts past the centre");
        let src = include_str!("../shaders/post.wgsl");
        let table = format!("array<f32, 6>({})", GHOSTS.iter().map(|g| format!("{g}")).collect::<Vec<_>>().join(", "));
        assert!(src.contains(&table), "post.wgsl's ghost table is not {table}");
    }
}
