//! `sim::shape_scale` — the third input to a terrain-bending technique: how the
//! environment and the spoken qualifiers resolve into a shape's actual size and
//! material. The raycast gives the anchor, [`shape_word`](crate::sim::shape_word)
//! gives the form + qualifiers, and this turns the structural factor `S` at the
//! anchor plus those qualifiers into concrete dimensions and a block material.
//!
//! Rules from the Earth shape spec (§4):
//! * **Height** scales ×2 for `Tall`, is pinned to 0.25 for `Flat`.
//! * **Radius** scales ×2 for `Wide`, ×0.5 for `Narrow`, ×1.5 for `Flat`.
//! * `Deep` makes the shape **hollow**.
//! * Base dimensions come from `S`: ridge height `S·2`, width `S·4`, travel `S·3`.
//! * **Material** by `S`: `>0.70` Deepslate, `>0.45` Stone, `>0.25` Cobblestone,
//!   else the anchor's own block. A Resonance-Tier upgrade bumps the material one
//!   tier (cobble→stone→deepslate) but only when `S > 0.55` (deeper rock can
//!   support the denser material without collapsing).
//!
//! Pure/std-only; composes with `shape_word::Qualifier`.

use crate::sim::shape_word::Qualifier;

/// `S` above which the Resonance-Tier material upgrade (and forced-deepslate)
/// holds without the shape partially collapsing.
pub const UPGRADE_MIN_S: f32 = 0.55;

/// Height multiplier from the qualifiers. `Tall` doubles; `Flat` pins the shape
/// low (0.25), overriding other height effects.
pub fn height_scale(qualifiers: &[Qualifier]) -> f32 {
    let mut s = 1.0_f32;
    let mut flat = false;
    for q in qualifiers {
        match q {
            Qualifier::Tall => s *= 2.0,
            Qualifier::Flat => flat = true,
            _ => {}
        }
    }
    if flat {
        0.25
    } else {
        s
    }
}

/// Radius/width multiplier from the qualifiers. `Wide` doubles, `Narrow` halves,
/// `Flat` spreads it 1.5×.
pub fn radius_scale(qualifiers: &[Qualifier]) -> f32 {
    let mut s = 1.0_f32;
    for q in qualifiers {
        match q {
            Qualifier::Wide => s *= 2.0,
            Qualifier::Narrow => s *= 0.5,
            Qualifier::Flat => s *= 1.5,
            _ => {}
        }
    }
    s
}

/// Whether the shape is hollow (a `Deep` qualifier).
pub fn is_hollow(qualifiers: &[Qualifier]) -> bool {
    qualifiers.iter().any(|q| matches!(q, Qualifier::Deep))
}

/// Concrete shape dimensions (world blocks / blocks-per-second).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeDimensions {
    pub height: f32,
    pub width: f32,
    pub travel_speed: f32,
}

/// Base dimensions from the structural factor `S` alone: `height = S·2`,
/// `width = S·4`, `travel = S·3`.
pub fn base_dimensions(s: f32) -> ShapeDimensions {
    let s = s.clamp(0.0, 1.0);
    ShapeDimensions { height: s * 2.0, width: s * 4.0, travel_speed: s * 3.0 }
}

/// Final dimensions: base (from `S`) scaled by the qualifier multipliers.
pub fn resolved_dimensions(s: f32, qualifiers: &[Qualifier]) -> ShapeDimensions {
    let base = base_dimensions(s);
    let h = height_scale(qualifiers);
    let r = radius_scale(qualifiers);
    ShapeDimensions {
        height: base.height * h,
        width: base.width * r,
        travel_speed: base.travel_speed,
    }
}

/// The material an Earth shape is built from, selected by structural depth `S`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EarthMaterial {
    /// `S ≤ 0.25`: build from whatever block the raycast hit.
    AnchorBlock,
    Cobblestone,
    Stone,
    Deepslate,
}

impl EarthMaterial {
    /// Structural integrity of the built material (0..1); `None` for the
    /// anchor-block passthrough since that depends on the hit block.
    pub fn integrity(self) -> Option<f32> {
        Some(match self {
            EarthMaterial::AnchorBlock => return None,
            EarthMaterial::Cobblestone => 0.60,
            EarthMaterial::Stone => 0.75,
            EarthMaterial::Deepslate => 0.88,
        })
    }

    fn rank(self) -> u8 {
        match self {
            EarthMaterial::AnchorBlock => 0,
            EarthMaterial::Cobblestone => 1,
            EarthMaterial::Stone => 2,
            EarthMaterial::Deepslate => 3,
        }
    }
}

/// Select the build material from the structural factor `S` at the anchor.
pub fn material_for_s(s: f32) -> EarthMaterial {
    if s > 0.70 {
        EarthMaterial::Deepslate
    } else if s > 0.45 {
        EarthMaterial::Stone
    } else if s > 0.25 {
        EarthMaterial::Cobblestone
    } else {
        EarthMaterial::AnchorBlock
    }
}

/// Apply a Resonance-Tier one-step material upgrade (cobble→stone→deepslate),
/// gated on `S > UPGRADE_MIN_S`. Below that threshold, or already at the top, the
/// material is unchanged. The anchor-block passthrough isn't upgraded.
pub fn upgrade_one_tier(material: EarthMaterial, s: f32) -> EarthMaterial {
    if s <= UPGRADE_MIN_S {
        return material;
    }
    match material {
        EarthMaterial::Cobblestone => EarthMaterial::Stone,
        EarthMaterial::Stone => EarthMaterial::Deepslate,
        other => other, // AnchorBlock and Deepslate unchanged
    }
}

/// Whether a forced-Deepslate override holds at this `S` (needs `S > UPGRADE_MIN_S`
/// or the shape partially collapses).
pub fn forced_deepslate_holds(s: f32) -> bool {
    s > UPGRADE_MIN_S
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::shape_word::Qualifier::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn height_scale_rules() {
        assert!(approx(height_scale(&[]), 1.0));
        assert!(approx(height_scale(&[Tall]), 2.0));
        assert!(approx(height_scale(&[Tall, Tall]), 4.0), "stacks");
        assert!(approx(height_scale(&[Flat]), 0.25), "flat pins low");
        assert!(approx(height_scale(&[Tall, Flat]), 0.25), "flat overrides tall");
    }

    #[test]
    fn radius_scale_rules() {
        assert!(approx(radius_scale(&[]), 1.0));
        assert!(approx(radius_scale(&[Wide]), 2.0));
        assert!(approx(radius_scale(&[Narrow]), 0.5));
        assert!(approx(radius_scale(&[Flat]), 1.5));
        assert!(approx(radius_scale(&[Wide, Narrow]), 1.0), "wide then narrow cancel");
    }

    #[test]
    fn hollow_flag() {
        assert!(is_hollow(&[Deep]));
        assert!(!is_hollow(&[Wide, Tall]));
    }

    #[test]
    fn base_dimensions_from_s() {
        let d = base_dimensions(1.0);
        assert!(approx(d.height, 2.0) && approx(d.width, 4.0) && approx(d.travel_speed, 3.0));
        let half = base_dimensions(0.5);
        assert!(approx(half.height, 1.0) && approx(half.width, 2.0));
    }

    #[test]
    fn resolved_dimensions_compose_env_and_qualifiers() {
        // "tall wide" at full S: height 2·2 = 4, width 4·2 = 8
        let d = resolved_dimensions(1.0, &[Tall, Wide]);
        assert!(approx(d.height, 4.0), "got {}", d.height);
        assert!(approx(d.width, 8.0), "got {}", d.width);
    }

    #[test]
    fn material_thresholds_match_spec() {
        assert_eq!(material_for_s(0.80), EarthMaterial::Deepslate);
        assert_eq!(material_for_s(0.50), EarthMaterial::Stone);
        assert_eq!(material_for_s(0.30), EarthMaterial::Cobblestone);
        assert_eq!(material_for_s(0.10), EarthMaterial::AnchorBlock);
        // boundary: exactly 0.70 is not yet Deepslate (strict >)
        assert_eq!(material_for_s(0.70), EarthMaterial::Stone);
    }

    #[test]
    fn resonance_tier_upgrade_is_gated_on_s() {
        // deep enough → one tier up
        assert_eq!(upgrade_one_tier(EarthMaterial::Cobblestone, 0.60), EarthMaterial::Stone);
        assert_eq!(upgrade_one_tier(EarthMaterial::Stone, 0.60), EarthMaterial::Deepslate);
        // too shallow → unchanged
        assert_eq!(upgrade_one_tier(EarthMaterial::Stone, 0.40), EarthMaterial::Stone);
        // already top / passthrough → unchanged
        assert_eq!(upgrade_one_tier(EarthMaterial::Deepslate, 0.90), EarthMaterial::Deepslate);
        assert_eq!(upgrade_one_tier(EarthMaterial::AnchorBlock, 0.90), EarthMaterial::AnchorBlock);
        assert!(!forced_deepslate_holds(0.5) && forced_deepslate_holds(0.6));
    }

    #[test]
    fn integrity_orders_by_material_strength() {
        assert!(EarthMaterial::Stone.integrity() < EarthMaterial::Deepslate.integrity());
        assert!(EarthMaterial::Cobblestone.integrity() < EarthMaterial::Stone.integrity());
        assert_eq!(EarthMaterial::AnchorBlock.integrity(), None);
        // ranks are consistent with the upgrade order
        assert!(EarthMaterial::Cobblestone.rank() < EarthMaterial::Deepslate.rank());
    }
}
