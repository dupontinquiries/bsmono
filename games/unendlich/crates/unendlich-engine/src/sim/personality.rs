//! Disposition — a creature's stable temperament, the per-individual weights
//! that make one dragon a territorial aggressor and another a wary opportunist.
//!
//! # Why it's derived, not stored
//!
//! A disposition never changes over a creature's life, so rather than carry it
//! in the rolled-back snapshot it is **derived deterministically** from
//! `(world_seed, mob_id)` via [`Disposition::from_seed`] — a pure hash, like
//! the mob brain's PRNG. Every peer and every replay computes the same
//! temperament for the same creature, with zero extra state to checksum.
//!
//! # Where it plugs in
//!
//! Disposition does not act on its own; it **weights goal priorities** in the
//! (forthcoming) goals layer. The same perceived threat yields different
//! choices per individual: an aggressive, territorial dragon scores
//! defend/attack high, while a cautious one scores adapt/avoid high — exactly
//! the aggressive-vs-passive axis the creature design calls for.
//! [`Disposition::confronts`] is the first such gate.

use crate::sim::fixed::Fixed;

/// A creature's temperament. Every trait is a [`Fixed`] in `[0, 1)`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Disposition {
    /// Meets threats with attack rather than avoidance.
    pub aggression: Fixed,
    /// Defends a home range; reacts to changes (new structures) within it.
    pub territoriality: Fixed,
    /// Keeps distance, flees when hurt, avoids risk.
    pub caution: Fixed,
    /// Investigates novelty — unseen landmarks, freshly built structures.
    pub curiosity: Fixed,
    /// Seeks food and hoard: raids villages, hunts livestock.
    pub greed: Fixed,
}

impl Disposition {
    /// Derive the stable temperament of a creature. Pure and deterministic.
    pub fn from_seed(world_seed: u64, mob_id: u64) -> Disposition {
        Disposition {
            aggression: trait_from(world_seed, mob_id, 0x01),
            territoriality: trait_from(world_seed, mob_id, 0x02),
            caution: trait_from(world_seed, mob_id, 0x03),
            curiosity: trait_from(world_seed, mob_id, 0x04),
            greed: trait_from(world_seed, mob_id, 0x05),
        }
    }

    /// Whether this creature meets an intrusion of strength `threat` (`[0,1]`)
    /// by confronting it (`true`) versus adapting/avoiding (`false`).
    ///
    /// `drive = threat · (aggression + territoriality)/2`; it confronts when the
    /// drive exceeds its `caution`. So a calm creature needs a much larger
    /// provocation to attack, and a trivial threat is shrugged off by all.
    pub fn confronts(&self, threat: Fixed) -> bool {
        let half = Fixed::from_frac(1, 2);
        let drive = threat * ((self.aggression + self.territoriality) * half);
        drive > self.caution
    }
}

/// One trait value in `[0, 1)`, hashed deterministically from seed/id/salt with
/// distinct salts so the five traits are independent.
fn trait_from(world_seed: u64, mob_id: u64, salt: u64) -> Fixed {
    let h = splitmix64(world_seed ^ mob_id.rotate_left(17) ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    // Top 16 bits → raw `[0, 65535]` → Fixed in `[0, 1)`.
    Fixed(((h >> 48) & 0xFFFF) as i64)
}

/// SplitMix64 finalizer (matches the mob brain's PRNG family).
fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_per_individual() {
        assert_eq!(Disposition::from_seed(42, 7), Disposition::from_seed(42, 7));
        assert_ne!(Disposition::from_seed(42, 7), Disposition::from_seed(42, 8));
        assert_ne!(Disposition::from_seed(1, 7), Disposition::from_seed(2, 7));
    }

    #[test]
    fn traits_in_unit_range() {
        for mid in 0..2000u64 {
            let d = Disposition::from_seed(99, mid);
            for t in [d.aggression, d.territoriality, d.caution, d.curiosity, d.greed] {
                assert!(t >= Fixed::ZERO && t < Fixed::ONE, "trait out of [0,1): {t:?}");
            }
        }
    }

    #[test]
    fn distribution_is_centered() {
        // Mean of each trait over many individuals should sit near 0.5.
        let n = 4096u64;
        let mut sum = Fixed::ZERO;
        for mid in 0..n {
            sum = sum + Disposition::from_seed(7, mid).aggression;
        }
        let mean = sum / Fixed::from_int(n as i64);
        assert!(mean > Fixed::from_frac(45, 100) && mean < Fixed::from_frac(55, 100), "mean {mean:?}");
    }

    #[test]
    fn confronts_reflects_temperament() {
        let f = |x: i64| Fixed::from_frac(x, 100);
        let bold = Disposition { aggression: f(90), territoriality: f(90), caution: f(10), curiosity: f(50), greed: f(50) };
        let timid = Disposition { aggression: f(10), territoriality: f(20), caution: f(90), curiosity: f(50), greed: f(50) };
        assert!(bold.confronts(f(90)), "a bold creature confronts a real threat");
        assert!(!timid.confronts(f(90)), "a timid creature adapts instead");
        assert!(!bold.confronts(f(10)), "even the bold shrug off a trivial threat");
    }
}
