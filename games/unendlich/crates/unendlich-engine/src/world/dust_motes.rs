//! `dust_motes` — ambient floating-particle density by biome + light.
//!
//! Worker-03 / WORLD-FX pure core, round 14. The faint dust you see drifting in
//! a light shaft: visibility scales with how lit the air is, with a per-biome
//! dustiness, and falls off with height (most motes hang near the ground). Dark
//! air shows none. Cosmetic FP32; pure and deterministic.
//!
//! Self-contained for `rustc --test`.
#![allow(dead_code)]

const HEIGHT_FALLOFF: f32 = 0.05;

/// Per-biome dustiness (unknown ids get a mid value).
pub fn biome_dust(biome: u8) -> f32 {
    match biome {
        0 => 0.9, // desert — dusty
        1 => 0.3, // forest — clean air
        2 => 0.5, // plains
        3 => 0.7, // cave / underground
        _ => 0.4,
    }
}

/// Visible mote density in [0,1] for a biome, ambient `light` (0..1), at
/// `height` above ground. Brighter air shows more motes; dark air shows none.
pub fn motes(biome: u8, light: f32, height: f32) -> f32 {
    let l = clamp01(light);
    let b = clamp01(biome_dust(biome));
    let hfall = 1.0 / (1.0 + height.max(0.0) * HEIGHT_FALLOFF); // (0,1]
    clamp01(b * l * hfall)
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brighter_shafts_show_more() {
        assert!(motes(2, 0.9, 0.0) > motes(2, 0.2, 0.0));
    }

    #[test]
    fn biome_scales_dustiness() {
        // Desert dustier than forest at equal light/height.
        assert!(motes(0, 0.8, 1.0) > motes(1, 0.8, 1.0));
    }

    #[test]
    fn height_falloff() {
        assert!(motes(2, 0.8, 0.0) > motes(2, 0.8, 100.0));
    }

    #[test]
    fn zero_in_dark() {
        for h in 0..10 {
            assert_eq!(motes(0, 0.0, h as f32), 0.0);
        }
    }

    #[test]
    fn in_unit_range() {
        for &b in &[0u8, 1, 2, 3, 200] {
            for li in 0..=10 {
                for h in 0..20 {
                    let v = motes(b, li as f32 / 10.0, h as f32 * 5.0);
                    assert!(v >= 0.0 && v <= 1.0);
                }
            }
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(motes(3, 0.6, 12.0), motes(3, 0.6, 12.0));
    }
}
