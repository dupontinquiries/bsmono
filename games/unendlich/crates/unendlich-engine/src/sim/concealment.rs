//! Concealment — how cover hides a target from being *seen*. Ducking into a
//! bush breaks up your silhouette; sinking into water does the same, and adds
//! murk and a refracting surface on top. This is a detectability multiplier the
//! perception layer applies after line-of-sight and water clarity: even down a
//! clear ray, a target tucked in cover is hard to pick out.
//!
//! It pairs with `sim::water_sight` (which already dims a target *through* water
//! by depth/turbidity/disturbance) — concealment is the cover the target is
//! *in*. A still creature submerged in murky water is very nearly invisible;
//! the same creature thrashing churns the surface (raising `disturbance` over in
//! water_sight) and gives itself away.

use crate::sim::fixed::Fixed;

/// The cover a target is in, from most exposed to most hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cover {
    /// Out in the open.
    None,
    /// Standing in shallow water — legs hidden, torso and ripples show.
    ShallowWater,
    /// In foliage / a bush — broken silhouette.
    Foliage,
    /// Mostly or fully submerged — murk and refraction do the rest.
    Submerged,
}

/// Fraction of detectability the cover REMOVES, in `[0,1]`. 0 = fully exposed,
/// 1 = effectively invisible. Tunable.
pub fn concealment(cover: Cover) -> Fixed {
    match cover {
        Cover::None => Fixed::ZERO,
        Cover::ShallowWater => Fixed::from_frac(40, 100),
        Cover::Foliage => Fixed::from_frac(60, 100),
        Cover::Submerged => Fixed::from_frac(85, 100),
    }
}

/// Detectability `= base_clarity · (1 − concealment)`: how clearly an observer
/// actually registers a target that it otherwise has a line on. `base_clarity`
/// is whatever the sight layer produced (e.g. `water_sight` clarity, or 1 in
/// open air).
pub fn detectability(base_clarity: Fixed, cover: Cover) -> Fixed {
    (base_clarity * (Fixed::ONE - concealment(cover))).clamp(Fixed::ZERO, Fixed::ONE)
}

/// Classify cover from whether the target is in foliage and how submerged it is
/// (`submersion` in `[0,1]`, e.g. from `physics::submersion`). Water dominates
/// foliage when deep.
pub fn cover_for(in_foliage: bool, submersion: Fixed) -> Cover {
    if submersion >= Fixed::from_frac(66, 100) {
        Cover::Submerged
    } else if submersion > Fixed::ZERO {
        Cover::ShallowWater
    } else if in_foliage {
        Cover::Foliage
    } else {
        Cover::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_ordering_makes_water_the_best_hide() {
        let none = concealment(Cover::None);
        let shallow = concealment(Cover::ShallowWater);
        let foliage = concealment(Cover::Foliage);
        let sub = concealment(Cover::Submerged);
        assert_eq!(none, Fixed::ZERO);
        assert!(shallow < foliage, "shallow water hides less than a bush");
        assert!(foliage < sub, "full submersion hides most");
    }

    #[test]
    fn detectability_drops_with_cover() {
        let clear = Fixed::ONE;
        assert_eq!(detectability(clear, Cover::None), Fixed::ONE, "open → fully visible");
        let exposed = detectability(clear, Cover::None);
        let hidden = detectability(clear, Cover::Submerged);
        assert!(hidden < exposed, "submerged target is far less detectable");
        // Submerged removes 85%, so detectability ≈ 0.15.
        assert!(hidden < Fixed::from_frac(2, 10), "deeply hidden: {hidden:?}");
    }

    #[test]
    fn cover_classification() {
        assert_eq!(cover_for(false, Fixed::ZERO), Cover::None);
        assert_eq!(cover_for(true, Fixed::ZERO), Cover::Foliage);
        assert_eq!(cover_for(false, Fixed::from_frac(3, 10)), Cover::ShallowWater);
        assert_eq!(cover_for(false, Fixed::from_frac(8, 10)), Cover::Submerged);
        assert_eq!(cover_for(true, Fixed::from_frac(8, 10)), Cover::Submerged, "deep water beats foliage");
    }
}
