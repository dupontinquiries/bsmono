//! material.rs — vehicle armour → surface response.
//!
//! Modelled on `unendlich-engine`'s `armor_reflect`: a single gameplay stat
//! drives how a surface answers light, so the thing you can read off the
//! HUD is the thing you can see on the car. A Behemoth should look like
//! plate; a RedSedan should look like a road car.
//!
//! Pure and deterministic, so the numbers can be asserted without a GPU.
//! The shader consumes them; it does not decide them.

use crate::vehicle::VehicleStats;

/// Surface response for the shader.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Material {
    /// How much sky the surface mirrors back, `[0, 1]`. Drives the rim and
    /// the strength of the specular highlight.
    pub reflectivity: f32,
    /// Specular exponent. Low is a broad soft sheen (dusty paint), high is
    /// a tight hard glint (polished plate).
    pub specular_power: f32,
}

/// Armour above which a surface is treated as fully plated. The heaviest
/// car in the game sits at 200; anything at or past this reads as armour
/// rather than bodywork.
const ARMOR_FULL: f32 = 200.0;
/// Bounds on the specular exponent. The floor keeps a highlight visible at
/// all — a matte car with no glint reads as a flat sticker at this
/// resolution — and the ceiling stops the highlight shrinking below a pixel
/// and flickering as the car rotates.
const MIN_SPEC: f32 = 8.0;
const MAX_SPEC: f32 = 96.0;

/// Material for a vehicle, from its armour stat.
///
/// Heavier armour is glossier and harder-highlighted. Both curves are
/// linear in normalised armour; there is no physical basis for a fancier
/// mapping here, and a monotonic one is what makes the stat legible.
pub fn for_vehicle(stats: &VehicleStats) -> Material {
    // `f32::clamp` does not launder a NaN: `NAN.clamp(0.0, 1.0)` is NaN.
    // This function looked guarded and produced a NaN reflectivity and
    // specular power, which ride in SceneUniforms and multiply straight
    // into the car's colour in the shader.
    //
    // I wrote this module and this bug in the same session as
    // games/blackstar-rs/docs/NON_FINITE_INPUTS.md, which names this exact
    // mechanism as the most common one in that survey.
    let armor = if stats.armor.is_finite() { stats.armor } else { 0.0 };
    let a = (armor / ARMOR_FULL).clamp(0.0, 1.0);
    Material {
        // Never zero: a completely non-reflective car loses its silhouette
        // against the ground at dusk, when the sky fill is all there is.
        reflectivity: 0.12 + 0.38 * a,
        specular_power: MIN_SPEC + (MAX_SPEC - MIN_SPEC) * a,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vehicle::CarKind;

    fn mat(kind: CarKind) -> Material {
        for_vehicle(&kind.stats())
    }

    #[test]
    fn heavier_armour_is_glossier_and_harder_highlighted() {
        // The whole point: the stat you read is the surface you see.
        let sedan = mat(CarKind::RedSedan);
        let behemoth = mat(CarKind::Behemoth);
        assert!(behemoth.reflectivity > sedan.reflectivity);
        assert!(behemoth.specular_power > sedan.specular_power);
    }

    #[test]
    fn every_car_keeps_some_reflectivity() {
        // A fully matte car loses its silhouette against the ground when
        // the sky fill is the only light left.
        for kind in [CarKind::Behemoth, CarKind::SixWheeler, CarKind::RedSedan, CarKind::Pillager] {
            let m = mat(kind);
            assert!(m.reflectivity > 0.05, "{kind:?} is too matte: {m:?}");
            assert!(m.reflectivity <= 1.0);
        }
    }

    #[test]
    fn the_specular_exponent_stays_in_a_renderable_band() {
        // Below the floor the highlight vanishes; above the ceiling it is
        // sub-pixel and flickers as the car rotates.
        for kind in [CarKind::Behemoth, CarKind::SixWheeler, CarKind::RedSedan, CarKind::Pillager] {
            let m = mat(kind);
            assert!((MIN_SPEC..=MAX_SPEC).contains(&m.specular_power), "{kind:?}: {m:?}");
        }
    }

    #[test]
    fn armour_past_the_cap_does_not_run_away() {
        let mut stats = CarKind::Behemoth.stats();
        stats.armor = 10_000.0;
        let m = for_vehicle(&stats);
        assert!(m.reflectivity <= 0.5 + 1e-6);
        assert_eq!(m.specular_power, MAX_SPEC);
    }

    #[test]
    fn a_non_finite_armour_still_gives_a_usable_surface() {
        // These values ride into the shader and multiply into the car's
        // colour; a NaN there is an unlit car, not a mis-lit one.
        for armor in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut stats = CarKind::RedSedan.stats();
            stats.armor = armor;
            let m = for_vehicle(&stats);
            assert!(m.reflectivity.is_finite() && m.specular_power.is_finite(), "armor={armor}: {m:?}");
            assert!(m.reflectivity > 0.05);
            assert!((MIN_SPEC..=MAX_SPEC).contains(&m.specular_power));
        }
    }

    #[test]
    fn zero_armour_is_still_a_valid_surface() {
        let mut stats = CarKind::RedSedan.stats();
        stats.armor = 0.0;
        let m = for_vehicle(&stats);
        assert!(m.reflectivity.is_finite() && m.specular_power >= MIN_SPEC);
    }
}

