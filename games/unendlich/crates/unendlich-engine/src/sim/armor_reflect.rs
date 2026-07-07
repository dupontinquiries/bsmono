//! Dragon MATERIAL gene — `armor` → surface reflectance (R28, DRG-dragons).
//!
//! `DRAGON_GENOME.md §MATERIAL`: *"armor → reflectivity + specular power
//! (shader; core `armor_reflect`). More armor = glossier/harder."*
//!
//! Pure: a normalised gene value in, two material parameters out. No RNG, no
//! I/O, no GPU — the shader consumes these scalars; producing them is all this
//! core does. Both outputs rise monotonically with `armor` (more armour reads
//! as a glossier, harder surface) and are clamped to documented ranges.

/// Material reflectance parameters derived from the `armor` gene.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArmorMaterial {
    /// Base (head-on) reflectance, an F0-style fraction. Organic dielectric
    /// range: soft hide barely reflects, hardened plate is glossier. Always
    /// within [`REFLECTIVITY_MIN`, `REFLECTIVITY_MAX`].
    pub reflectivity: f32,
    /// Blinn–Phong-style specular exponent: larger = tighter, harder-looking
    /// highlight. Always within [`SPECULAR_POWER_MIN`, `SPECULAR_POWER_MAX`].
    pub specular_power: f32,
}

/// Reflectivity at `armor = 0` (soft hide — minimally reflective dielectric).
pub const REFLECTIVITY_MIN: f32 = 0.02;
/// Reflectivity at `armor = 1` (hardened, glossy plate).
pub const REFLECTIVITY_MAX: f32 = 0.10;
/// Specular exponent at `armor = 0` (broad, soft highlight).
pub const SPECULAR_POWER_MIN: f32 = 8.0;
/// Specular exponent at `armor = 1` (tight, hard highlight).
pub const SPECULAR_POWER_MAX: f32 = 128.0;

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    crate::util::mathx::lerp(a, b, t) // r342 dedup
}

/// Express the `armor` gene (nominally 0..1) as material reflectance.
///
/// The gene is clamped to [0,1] first, so out-of-range inputs (e.g. a breeding
/// blend that overshoots) still yield in-range outputs. Monotonically
/// non-decreasing in `armor` for both fields.
pub fn express(armor: f32) -> ArmorMaterial {
    let a = armor.clamp(0.0, 1.0);
    ArmorMaterial {
        reflectivity: lerp(REFLECTIVITY_MIN, REFLECTIVITY_MAX, a),
        specular_power: lerp(SPECULAR_POWER_MIN, SPECULAR_POWER_MAX, a),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn endpoints_and_midpoint() {
        let lo = express(0.0);
        assert!(approx(lo.reflectivity, REFLECTIVITY_MIN));
        assert!(approx(lo.specular_power, SPECULAR_POWER_MIN));

        let hi = express(1.0);
        assert!(approx(hi.reflectivity, REFLECTIVITY_MAX));
        assert!(approx(hi.specular_power, SPECULAR_POWER_MAX));

        let mid = express(0.5);
        assert!(approx(mid.reflectivity, (REFLECTIVITY_MIN + REFLECTIVITY_MAX) * 0.5));
        assert!(approx(mid.specular_power, (SPECULAR_POWER_MIN + SPECULAR_POWER_MAX) * 0.5));
    }

    #[test]
    fn out_of_range_gene_clamps_into_documented_range() {
        for &g in &[-5.0_f32, -0.001, 1.001, 9.0] {
            let m = express(g);
            assert!(m.reflectivity >= REFLECTIVITY_MIN && m.reflectivity <= REFLECTIVITY_MAX);
            assert!(m.specular_power >= SPECULAR_POWER_MIN && m.specular_power <= SPECULAR_POWER_MAX);
        }
    }

    #[test]
    fn monotonic_finite_and_in_range() {
        let mut prev = express(0.0);
        for i in 0..=1000 {
            let g = i as f32 / 1000.0;
            let m = express(g);
            assert!(m.reflectivity.is_finite() && m.specular_power.is_finite());
            assert!(m.reflectivity >= REFLECTIVITY_MIN && m.reflectivity <= REFLECTIVITY_MAX);
            assert!(m.specular_power >= SPECULAR_POWER_MIN && m.specular_power <= SPECULAR_POWER_MAX);
            // Non-decreasing in the gene (more armor = glossier/harder).
            assert!(m.reflectivity + 1e-6 >= prev.reflectivity);
            assert!(m.specular_power + 1e-4 >= prev.specular_power);
            prev = m;
        }
    }
}
