//! Exotic plant traits — rare, heritable specializations beyond the common
//! [`crate::world::plant_genetics`] gene set.
//!
//! Exotics fall into three mechanisms, and the [`ExoticClass`] tells a consumer
//! which system handles a given trait:
//!
//! - **Resistance** — read by other systems when they act on the plant.
//!   `FireResist`: when a Fire/heat `emission` deposit lands on the plant's cell,
//!   the lifecycle's wither/death check attenuates it. (A fire-tolerant grove.)
//! - **Production** — the plant outputs something over time.
//!   `FruitBearing`: a mature plant drops a fruit item on a cadence (the items /
//!   drops path — this thread). `OreBloom`: slowly accretes ore into nearby
//!   substrate (an Earth-domain structural edit — rare/slow by design; see perf).
//! - **Aura** — the plant becomes an `emission`-style *source* affecting nearby
//!   creatures, exactly as a dragon's breath deposits into the Discharge layer.
//!   `GravityWell` (lighter footfalls), `Swiftcap` (haste), `Healbloom` (radiant
//!   heal), `Sporefog{Healing|Poison}` (a visible spore cloud, curative or toxic).
//!
//! This module is the **pure core for the aura group** — the typed creature
//! effect, its radius/cadence, and a distance test — plus exotic heredity. The
//! resistance/production mechanics are hooks documented above; their effects land
//! in the lifecycle, the items path, and (for fog/aura *visuals*) the render
//! particle/froxel layer on the shaders thread.
//!
//! ## Determinism / units
//!
//! Scales are integer **per-mille** (1000 = ×1.0) so this stays exact and
//! rollback-safe with no float; a physics consumer converts to `Fixed` via
//! `Fixed::from_frac(permille, 1000)`. Health is signed integer per emission.
//!
//! ## Perf (mobile-first)
//!
//! Auras are *periodic* (`period_ticks`) and *bounded* (a small radius) around
//! *rare* plants, so the per-tick cost is tiny — and the fog **particles** are a
//! render concern, not sim. The expensive exotics are the structural ones:
//! `OreBloom` mints block edits → `structural_dirty` → rebuild/upload/persist, so
//! it must stay slow and rare. `FruitBearing` drops items (cheap) rather than
//! editing the world. The aura math here is a handful of integer ops.

/// Which world system realizes an exotic trait.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExoticClass {
    /// Read when something acts on the plant (e.g. fire/heat).
    Resistance,
    /// Emits items or accretes blocks over time.
    Production,
    /// Emits a creature-affecting field, like a dragon's breath.
    Aura,
}

/// Polarity of a fungal fog cloud.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SporeKind {
    Healing,
    Poison,
}

/// A rare specialization a plant may carry (in addition to its common genome).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExoticTrait {
    /// Survives Fire/heat that would wither an ordinary plant.
    FireResist,
    /// Slowly accretes ore into nearby substrate (Earth domain, structural).
    OreBloom,
    /// Drops a fruit item on a cadence when mature (items / drops path).
    FruitBearing,
    /// Lighter gravity for creatures in range.
    GravityWell,
    /// Haste for creatures in range.
    Swiftcap,
    /// Radiant heal for creatures in range (no visible fog).
    Healbloom,
    /// A visible spore cloud — curative or toxic.
    Sporefog(SporeKind),
}

/// Every exotic, for spawn-weighting and mutation rolls.
pub const EXOTICS: [ExoticTrait; 8] = [
    ExoticTrait::FireResist,
    ExoticTrait::OreBloom,
    ExoticTrait::FruitBearing,
    ExoticTrait::GravityWell,
    ExoticTrait::Swiftcap,
    ExoticTrait::Healbloom,
    ExoticTrait::Sporefog(SporeKind::Healing),
    ExoticTrait::Sporefog(SporeKind::Poison),
];

/// The typed effect an aura applies to a creature in range. Scales are per-mille
/// (1000 = ×1.0); `Health` is signed (+heal / −poison) per emission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuraEffect {
    /// Gravity multiplier, per-mille (<1000 reduces fall pull).
    GravityScale(u16),
    /// Move-speed multiplier, per-mille (>1000 speeds up).
    SpeedScale(u16),
    /// Hit-points changed per emission (positive heals, negative harms).
    Health(i32),
}

/// A plant's emitted aura: a typed effect over a radius, fired on a cadence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Aura {
    pub effect: AuraEffect,
    /// Chebyshev radius in blocks within which the effect applies.
    pub radius: i32,
    /// Ticks between emissions.
    pub period_ticks: u32,
    /// Whether this manifests as a visible fog cloud (render particle layer).
    pub fog: bool,
}

impl Aura {
    /// The effect at squared distance `dist_sq` from the plant — `Some` inside
    /// the radius, `None` outside. Squared distance keeps this sqrt-free (the sim
    /// is sqrt/trig-free). A smooth falloff is deferred; today it is a clean ball.
    pub fn effect_at(&self, dist_sq: i64) -> Option<AuraEffect> {
        let r = self.radius.max(0) as i64;
        if dist_sq <= r * r {
            Some(self.effect)
        } else {
            None
        }
    }
}

impl ExoticTrait {
    /// Which system handles this trait.
    pub fn class(self) -> ExoticClass {
        match self {
            ExoticTrait::FireResist => ExoticClass::Resistance,
            ExoticTrait::OreBloom | ExoticTrait::FruitBearing => ExoticClass::Production,
            ExoticTrait::GravityWell
            | ExoticTrait::Swiftcap
            | ExoticTrait::Healbloom
            | ExoticTrait::Sporefog(_) => ExoticClass::Aura,
        }
    }

    /// The aura this trait emits at the given vigor (`0..=1000` per-mille; a more
    /// vigorous plant projects a larger radius). `None` for non-aura traits.
    pub fn aura(self, vigor_permille: u16) -> Option<Aura> {
        let v = vigor_permille.min(1000) as i32;
        // radius = base + extra * vigor
        let radius = |base: i32, extra: i32| base + extra * v / 1000;
        match self {
            ExoticTrait::GravityWell => Some(Aura {
                effect: AuraEffect::GravityScale(500), // half gravity at the core
                radius: radius(3, 4),
                period_ticks: 20,
                fog: false,
            }),
            ExoticTrait::Swiftcap => Some(Aura {
                effect: AuraEffect::SpeedScale(1500), // +50% speed
                radius: radius(3, 4),
                period_ticks: 20,
                fog: false,
            }),
            ExoticTrait::Healbloom => Some(Aura {
                effect: AuraEffect::Health(1),
                radius: radius(2, 3),
                period_ticks: 40,
                fog: false,
            }),
            ExoticTrait::Sporefog(SporeKind::Healing) => Some(Aura {
                effect: AuraEffect::Health(1),
                radius: radius(4, 6),
                period_ticks: 30,
                fog: true,
            }),
            ExoticTrait::Sporefog(SporeKind::Poison) => Some(Aura {
                effect: AuraEffect::Health(-2),
                radius: radius(4, 6),
                period_ticks: 30,
                fog: true,
            }),
            // Resistance / Production traits emit no aura.
            ExoticTrait::FireResist | ExoticTrait::OreBloom | ExoticTrait::FruitBearing => None,
        }
    }
}

#[inline]
fn mix(seed: u64) -> u64 { crate::util::hash_util::mix64(seed) } // r193 4.1

#[inline]
fn unit(seed: u64) -> f32 {
    (mix(seed) >> 40) as f32 / (1u64 << 24) as f32
}

/// Inherit an exotic for an offspring from its two parents' exotics.
///
/// Exotics are rare and lean recessive-ish: shared by both parents → very likely
/// to carry; one parent → unlikely; neither → a tiny spontaneous-mutation chance
/// so new exotics can appear in a pristine population. Deterministic in `seed`.
pub fn inherit_exotic(
    a: Option<ExoticTrait>,
    b: Option<ExoticTrait>,
    seed: u64,
) -> Option<ExoticTrait> {
    let u = unit(seed);
    match (a, b) {
        (Some(x), Some(y)) if x == y => (u < 0.80).then_some(x),
        (Some(x), Some(y)) => {
            // Different exotics: occasionally one wins, else neither.
            if u < 0.40 {
                Some(if mix(seed ^ 0x55) & 1 == 0 { x } else { y })
            } else {
                None
            }
        }
        (Some(x), None) | (None, Some(x)) => (u < 0.25).then_some(x),
        (None, None) => {
            // Spontaneous mutation: rare new exotic out of a plain population.
            (u < 0.01).then_some(EXOTICS[(mix(seed ^ 0xE0) as usize) % EXOTICS.len()])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_routing_is_correct() {
        assert_eq!(ExoticTrait::FireResist.class(), ExoticClass::Resistance);
        assert_eq!(ExoticTrait::OreBloom.class(), ExoticClass::Production);
        assert_eq!(ExoticTrait::FruitBearing.class(), ExoticClass::Production);
        for t in [
            ExoticTrait::GravityWell,
            ExoticTrait::Swiftcap,
            ExoticTrait::Healbloom,
            ExoticTrait::Sporefog(SporeKind::Healing),
            ExoticTrait::Sporefog(SporeKind::Poison),
        ] {
            assert_eq!(t.class(), ExoticClass::Aura);
        }
    }

    #[test]
    fn only_aura_traits_emit_auras() {
        assert!(ExoticTrait::FireResist.aura(1000).is_none());
        assert!(ExoticTrait::OreBloom.aura(1000).is_none());
        assert!(ExoticTrait::FruitBearing.aura(1000).is_none());
        assert!(ExoticTrait::GravityWell.aura(1000).is_some());
        assert!(ExoticTrait::Healbloom.aura(0).is_some());
    }

    #[test]
    fn gravity_lowers_and_swift_raises() {
        match ExoticTrait::GravityWell.aura(1000).unwrap().effect {
            AuraEffect::GravityScale(p) => assert!(p < 1000, "gravity well should reduce pull"),
            other => panic!("expected GravityScale, got {:?}", other),
        }
        match ExoticTrait::Swiftcap.aura(1000).unwrap().effect {
            AuraEffect::SpeedScale(p) => assert!(p > 1000, "swiftcap should speed up"),
            other => panic!("expected SpeedScale, got {:?}", other),
        }
    }

    #[test]
    fn fungal_fog_polarity_and_flag() {
        let heal = ExoticTrait::Sporefog(SporeKind::Healing).aura(1000).unwrap();
        let pois = ExoticTrait::Sporefog(SporeKind::Poison).aura(1000).unwrap();
        assert!(heal.fog && pois.fog, "spore clouds are visible fog");
        match (heal.effect, pois.effect) {
            (AuraEffect::Health(h), AuraEffect::Health(p)) => {
                assert!(h > 0, "healing spores heal");
                assert!(p < 0, "poison spores harm");
            }
            _ => panic!("spore effects should be Health"),
        }
        // A non-fog aura stays invisible.
        assert!(!ExoticTrait::Healbloom.aura(1000).unwrap().fog);
    }

    #[test]
    fn aura_applies_only_within_radius() {
        let a = ExoticTrait::Swiftcap.aura(1000).unwrap(); // radius 3+4 = 7
        assert_eq!(a.radius, 7);
        let r = a.radius as i64;
        assert!(a.effect_at(r * r).is_some(), "edge is inside");
        assert!(a.effect_at(r * r + 1).is_none(), "just past the edge is outside");
        assert!(a.effect_at(0).is_some(), "center is inside");
    }

    #[test]
    fn vigor_grows_the_radius() {
        let weak = ExoticTrait::GravityWell.aura(0).unwrap().radius; // base 3
        let strong = ExoticTrait::GravityWell.aura(1000).unwrap().radius; // 3 + 4
        assert_eq!(weak, 3);
        assert_eq!(strong, 7);
        assert!(strong > weak);
    }

    #[test]
    fn shared_parents_usually_pass_the_exotic() {
        // Both parents FireResist → most children carry it; neither parent →
        // almost none do. Statistical over many deterministic seeds.
        let both = ExoticTrait::FireResist;
        let mut shared = 0;
        let mut spontaneous = 0;
        for s in 0..2000u64 {
            if inherit_exotic(Some(both), Some(both), s).is_some() {
                shared += 1;
            }
            if inherit_exotic(None, None, s).is_some() {
                spontaneous += 1;
            }
        }
        assert!(shared > 1400, "shared exotic should pass most of the time, got {shared}");
        assert!(spontaneous < 100, "spontaneous mutation should be rare, got {spontaneous}");
    }

    #[test]
    fn single_parent_inheritance_is_uncommon() {
        let mut carried = 0;
        for s in 0..2000u64 {
            if inherit_exotic(Some(ExoticTrait::Swiftcap), None, s).is_some() {
                carried += 1;
            }
        }
        // ~25% expected; bound loosely but well below the shared case.
        assert!(carried > 350 && carried < 650, "single-parent ~25%, got {carried}");
    }

    #[test]
    fn inherit_is_deterministic() {
        let a = Some(ExoticTrait::Healbloom);
        let b = Some(ExoticTrait::GravityWell);
        for s in [1u64, 99, 123_456, u64::MAX] {
            assert_eq!(inherit_exotic(a, b, s), inherit_exotic(a, b, s));
        }
    }
}
