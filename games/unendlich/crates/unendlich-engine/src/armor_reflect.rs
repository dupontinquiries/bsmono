//! armor_reflect.rs — pure armor gene → shader material.
//!
//! Maps an armor gene (and a gloss scale) to a surface material: more armor
//! means a glossier, harder-highlighted surface. Pure, deterministic, panic-free
//! (R1/R11/R21).

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Material {
    /// Specular reflectivity `[0,1]` (matte → mirror-ish).
    pub reflectivity: f32,
    /// Specular exponent (broad highlight → tight, hard highlight).
    pub specular_power: f32,
}

/// Max specular exponent.
const MAX_SPEC: f32 = 256.0;

/// `armor` and `scale_gloss` are genes in `[0,1]`. Reflectivity rises with
/// armor; the specular exponent hardens with armor and is further sharpened by
/// gloss (micro-spec). Specular power is bounded to `MAX_SPEC`.
pub fn armor_reflect(armor: f32, scale_gloss: f32) -> Material {
    let a = armor.max(0.0).min(1.0);
    let g = scale_gloss.max(0.0).min(1.0);

    // matte dielectric base (~0.02) -> highly reflective plate
    let reflectivity = (0.02 + 0.85 * a).max(0.0).min(1.0);

    // armor hardens the highlight; gloss adds a sharper micro-specular term
    let base_power = 4.0 + 120.0 * a;
    let micro = 120.0 * g;
    let specular_power = (base_power + micro).min(MAX_SPEC);

    Material { reflectivity, specular_power }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn armor_zero_is_matte() {
        let m = armor_reflect(0.0, 0.0);
        assert!(m.reflectivity < 0.1, "armor 0 should be matte, got {}", m.reflectivity);
    }

    #[test]
    fn more_armor_more_reflectivity() {
        let lo = armor_reflect(0.2, 0.0).reflectivity;
        let hi = armor_reflect(0.9, 0.0).reflectivity;
        assert!(hi > lo, "reflectivity not rising with armor");
    }

    #[test]
    fn specular_power_bounded() {
        let mut a = 0.0f32;
        while a <= 1.0 {
            let mut g = 0.0f32;
            while g <= 1.0 {
                let m = armor_reflect(a, g);
                assert!(m.specular_power >= 4.0 && m.specular_power <= MAX_SPEC, "spec oob {}", m.specular_power);
                g += 0.1;
            }
            a += 0.1;
        }
    }

    #[test]
    fn gloss_adds_micro_spec() {
        for &a in &[0.0f32, 0.5, 1.0] {
            let dull = armor_reflect(a, 0.0).specular_power;
            let shiny = armor_reflect(a, 1.0).specular_power;
            assert!(shiny > dull, "gloss did not add micro-spec at armor {}", a);
        }
    }

    #[test]
    fn monotonic_in_armor() {
        let mut prev_r = -1.0f32;
        let mut prev_s = -1.0f32;
        let mut a = 0.0f32;
        while a <= 1.0 {
            let m = armor_reflect(a, 0.3);
            assert!(m.reflectivity >= prev_r - 1e-6, "reflectivity not monotone");
            assert!(m.specular_power >= prev_s - 1e-6, "spec not monotone");
            prev_r = m.reflectivity;
            prev_s = m.specular_power;
            a += 0.02;
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(armor_reflect(0.6, 0.4), armor_reflect(0.6, 0.4));
    }
}
