//! lod_morph.rs — pure geomorph blend weight for smooth LOD transitions.
//!
//! Smoothstep weight `0..1` over `[lo, hi]` used to interpolate vertex positions
//! between two LODs (geomorphing) so vertices don't pop on a LOD switch. Pure,
//! deterministic, panic-free (R1/R11/R21).

/// Geomorph weight at `dist`: `0` below `lo` (full fine LOD), `1` above `hi`
/// (full coarse LOD), smoothstep between. `hi` is guarded to exceed `lo`. In
/// `[0,1]`.
pub fn morph(dist: f32, lo: f32, hi: f32) -> f32 {
    let h = hi.max(lo + 1e-4);
    if dist <= lo {
        0.0
    } else if dist >= h {
        1.0
    } else {
        let t = (dist - lo) / (h - lo);
        (t * t * (3.0 - 2.0 * t)).max(0.0).min(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn below_lo_is_zero() {
        assert_eq!(morph(5.0, 10.0, 20.0), 0.0);
        assert_eq!(morph(10.0, 10.0, 20.0), 0.0);
    }

    #[test]
    fn above_hi_is_one() {
        assert_eq!(morph(20.0, 10.0, 20.0), 1.0);
        assert_eq!(morph(99.0, 10.0, 20.0), 1.0);
    }

    #[test]
    fn smooth_mid() {
        assert!((morph(15.0, 10.0, 20.0) - 0.5).abs() < 1e-5);
        let mut prev = -1.0f32;
        let mut d = 10.0f32;
        while d <= 20.0 {
            let m = morph(d, 10.0, 20.0);
            assert!(m >= prev - 1e-6, "not monotone at {}", d);
            prev = m;
            d += 0.1;
        }
    }

    #[test]
    fn clamped_unit_range() {
        let mut seed = 0x2207_0001u32;
        let mut lcg = || { seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); (seed >> 8) as f32 / ((1u32 << 24) as f32) };
        for _ in 0..2000 {
            let m = morph(lcg() * 100.0, lcg() * 50.0, lcg() * 50.0 + 25.0);
            assert!((0.0..=1.0).contains(&m), "morph oob {}", m);
        }
    }

    #[test]
    fn hi_guard() {
        let m = morph(15.0, 20.0, 10.0);
        assert!(m.is_finite() && (0.0..=1.0).contains(&m));
    }

    #[test]
    fn deterministic() {
        assert_eq!(morph(13.0, 10.0, 20.0), morph(13.0, 10.0, 20.0));
    }
}
