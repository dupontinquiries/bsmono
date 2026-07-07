//! sim::technique_catalogue — named bending techniques (spec §3.2, §5; panel §5).
//!
//! `gesture_resolve` turns a `[stance]+[gesture]+[directional]+element` cast into
//! a `TechniqueShape` (scale/range/radial). This adds the *identity* layer: a
//! curated catalogue of **named** techniques keyed by that same signature, each
//! with a Resonance cost, a minimum Resonance tier, and an effect category — so
//! the move set has concrete, castable moves (Tidal Surge, Glacial Form, Stone
//! Pillar, Fire Aura, Abyssal Vortex …) rather than only anonymous shapes.
//! `identify` maps a cast to its technique; expensive signature techniques have
//! their cost waived at Tier 3 (spec §4.1). Pure, deterministic, std-only.

use crate::sim::gesture_resolve::{Directional, Element, Gesture, Stance};

/// What a technique does, at a glance (drives VFX / effect routing).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectKind {
    Projectile,
    Wave,
    Barrier,
    Aura,
    Construct,
    Super,
}

/// A named technique and its cast signature.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Technique {
    pub name: &'static str,
    pub element: Element,
    pub stance: Stance,
    pub gesture: Gesture,
    pub directional: Directional,
    /// Resonance spent on use (spec §4.2: ~0.5–1.0 of a tier).
    pub resonance_cost: f32,
    /// Minimum Resonance tier to cast.
    pub min_tier: u8,
    pub effect: EffectKind,
    /// A signature/super technique whose cost is waived at Tier 3.
    pub expensive: bool,
}

/// The Tier at which expensive signature techniques cost no Resonance.
pub const FREE_CAST_TIER: u8 = 3;

/// The curated technique set. Signatures follow spec §3.2 and character-panel §5.
pub static CATALOGUE: &[Technique] = &[
    // ── Water ────────────────────────────────────────────────────────────────
    Technique {
        name: "Water Bolt",
        element: Element::Water,
        stance: Stance::Surge,
        gesture: Gesture::Strike,
        directional: Directional::Push,
        resonance_cost: 0.5,
        min_tier: 1,
        effect: EffectKind::Projectile,
        expensive: false,
    },
    Technique {
        name: "Tidal Surge",
        element: Element::Water,
        stance: Stance::Open,
        gesture: Gesture::Sustain,
        directional: Directional::Sweep,
        resonance_cost: 0.7,
        min_tier: 1,
        effect: EffectKind::Wave,
        expensive: false,
    },
    Technique {
        name: "Surge Lance",
        element: Element::Water,
        stance: Stance::Open,
        gesture: Gesture::ChargeRelease,
        directional: Directional::Push,
        resonance_cost: 0.6,
        min_tier: 1,
        effect: EffectKind::Projectile,
        expensive: false,
    },
    Technique {
        name: "Glacial Form",
        element: Element::Water,
        stance: Stance::Rooted,
        gesture: Gesture::Sustain,
        directional: Directional::Ground,
        resonance_cost: 1.0,
        min_tier: 2,
        effect: EffectKind::Barrier,
        expensive: true,
    },
    Technique {
        name: "Abyssal Vortex",
        element: Element::Water,
        stance: Stance::Rooted,
        gesture: Gesture::Sustain,
        directional: Directional::Radial,
        resonance_cost: 1.0,
        min_tier: 3,
        effect: EffectKind::Super,
        expensive: true,
    },
    // ── Air ──────────────────────────────────────────────────────────────────
    Technique {
        name: "Air Blade",
        element: Element::Air,
        stance: Stance::Flow,
        gesture: Gesture::Sustain,
        directional: Directional::Sweep,
        resonance_cost: 0.6,
        min_tier: 1,
        effect: EffectKind::Wave,
        expensive: false,
    },
    // ── Earth ────────────────────────────────────────────────────────────────
    Technique {
        name: "Stone Pillar",
        element: Element::Earth,
        stance: Stance::Rooted,
        gesture: Gesture::ChargeRelease,
        directional: Directional::Lift,
        resonance_cost: 0.8,
        min_tier: 1,
        effect: EffectKind::Construct,
        expensive: false,
    },
    // ── Fire ─────────────────────────────────────────────────────────────────
    Technique {
        name: "Fire Aura",
        element: Element::Fire,
        stance: Stance::Aerial,
        gesture: Gesture::Sustain,
        directional: Directional::Radial,
        resonance_cost: 0.7,
        min_tier: 2,
        effect: EffectKind::Aura,
        expensive: false,
    },
];

/// The named technique matching an exact cast signature, if any.
pub fn identify(
    element: Element,
    stance: Stance,
    gesture: Gesture,
    directional: Directional,
) -> Option<&'static Technique> {
    CATALOGUE.iter().find(|t| {
        t.element == element && t.stance == stance && t.gesture == gesture && t.directional == directional
    })
}

/// All techniques of an element, in catalogue order.
pub fn techniques_for(element: Element) -> Vec<&'static Technique> {
    CATALOGUE.iter().filter(|t| t.element == element).collect()
}

/// Can this technique be cast at the given Resonance `tier`?
pub fn castable(t: &Technique, tier: u8) -> bool {
    tier >= t.min_tier
}

/// The Resonance actually spent at `tier` — expensive signature techniques are
/// free at Tier 3 (spec §4.1).
pub fn effective_cost(t: &Technique, tier: u8) -> f32 {
    if t.expensive && tier >= FREE_CAST_TIER {
        0.0
    } else {
        t.resonance_cost
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_identify_their_technique() {
        let t = identify(Element::Water, Stance::Surge, Gesture::Strike, Directional::Push).unwrap();
        assert_eq!(t.name, "Water Bolt");
        let g = identify(Element::Water, Stance::Rooted, Gesture::Sustain, Directional::Ground).unwrap();
        assert_eq!(g.name, "Glacial Form");
        let s = identify(Element::Earth, Stance::Rooted, Gesture::ChargeRelease, Directional::Lift).unwrap();
        assert_eq!(s.name, "Stone Pillar");
    }

    #[test]
    fn an_unknown_signature_has_no_named_technique() {
        assert!(identify(Element::Fire, Stance::Rooted, Gesture::Draw, Directional::Pull).is_none());
    }

    #[test]
    fn techniques_group_by_element() {
        let water = techniques_for(Element::Water);
        assert_eq!(water.len(), 5);
        assert!(water.iter().all(|t| t.element == Element::Water));
        assert_eq!(techniques_for(Element::Air).len(), 1);
    }

    #[test]
    fn tier_gates_casting() {
        let vortex = identify(Element::Water, Stance::Rooted, Gesture::Sustain, Directional::Radial).unwrap();
        assert!(!castable(vortex, 2), "Abyssal Vortex needs Tier 3");
        assert!(castable(vortex, 3));
        let bolt = identify(Element::Water, Stance::Surge, Gesture::Strike, Directional::Push).unwrap();
        assert!(castable(bolt, 1));
    }

    #[test]
    fn expensive_signatures_are_free_at_tier_three() {
        let vortex = identify(Element::Water, Stance::Rooted, Gesture::Sustain, Directional::Radial).unwrap();
        assert_eq!(effective_cost(vortex, 2), 1.0);
        assert_eq!(effective_cost(vortex, 3), 0.0, "waived at Tier 3");
        // cheap techniques still cost at Tier 3
        let bolt = identify(Element::Water, Stance::Surge, Gesture::Strike, Directional::Push).unwrap();
        assert_eq!(effective_cost(bolt, 3), 0.5);
    }

    #[test]
    fn every_catalogue_entry_round_trips_through_identify() {
        for t in CATALOGUE {
            let found = identify(t.element, t.stance, t.gesture, t.directional).unwrap();
            assert_eq!(found.name, t.name);
        }
    }

    #[test]
    fn deterministic() {
        let a = identify(Element::Air, Stance::Flow, Gesture::Sustain, Directional::Sweep);
        let b = identify(Element::Air, Stance::Flow, Gesture::Sustain, Directional::Sweep);
        assert_eq!(a, b);
    }
}
