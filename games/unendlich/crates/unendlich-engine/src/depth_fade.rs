//! depth_fade.rs — pure soft-particle fade by depth difference to the scene.
//!
//! Fades a particle out as it approaches the opaque scene depth behind it, so
//! billboards don't show a hard intersection seam against geometry. Pure,
//! deterministic, panic-free (R1/R11/R21). Result in `[0,1]` (1 = full opacity).
//!
//! Convention: larger depth = farther. `scene_z - particle_z > 0` means the
//! scene is behind the particle (particle in front).

/// Soft-particle opacity for a particle at depth `particle_z` against opaque
/// scene depth `scene_z`, with a `fade` distance. `0` when touching the surface,
/// ramping to `1` once the gap reaches `fade`. `fade` guarded `> 0`.
pub fn soft(particle_z: f32, scene_z: f32, fade: f32) -> f32 {
    let f = fade.max(1e-4);
    let gap = scene_z - particle_z; // positive when particle is in front of scene
    crate::util::mathx::saturate(gap / f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn far_from_surface_is_full() {
        assert!((soft(0.0, 100.0, 2.0) - 1.0).abs() < 1e-6, "deep in front -> full opacity");
    }

    #[test]
    fn touching_is_zero() {
        assert_eq!(soft(10.0, 10.0, 2.0), 0.0, "at the surface -> faded out");
        assert_eq!(soft(12.0, 10.0, 2.0), 0.0, "behind the surface -> 0");
    }

    #[test]
    fn fade_scales() {
        // a bigger fade distance gives a smaller opacity at a fixed gap
        let gap = 1.0;
        assert!(soft(0.0, gap, 4.0) <= soft(0.0, gap, 1.0));
    }

    #[test]
    fn clamped_unit_range() {
        let mut seed = 0x2307_0001u32;
        let mut lcg = || { seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); (seed >> 8) as f32 / ((1u32 << 24) as f32) };
        for _ in 0..2000 {
            let s = soft(lcg() * 100.0, lcg() * 100.0, lcg() * 5.0);
            assert!((0.0..=1.0).contains(&s), "soft oob {}", s);
        }
    }

    #[test]
    fn fade_guard() {
        let s = soft(0.0, 1.0, 0.0);
        assert!(s.is_finite() && (0.0..=1.0).contains(&s));
        let t = soft(0.0, 1.0, -3.0);
        assert!(t.is_finite() && (0.0..=1.0).contains(&t));
    }

    #[test]
    fn deterministic() {
        assert_eq!(soft(1.0, 3.0, 2.0), soft(1.0, 3.0, 2.0));
    }
}
