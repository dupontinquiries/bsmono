//! Elemental affinity — a heritable dragon trait that decides its breath
//! (fire, ice, wind, …). Most dragons carry exactly one element; carrying none,
//! two, or all three is progressively rarer. Crossbreeding usually passes a
//! single parent element to the offspring, occasionally the whole inherited set,
//! and rarely throws a fresh mutation — so a population can slowly accumulate the
//! prized multi-element bloodlines instead of having them sprinkled at random.
//!
//! Pure and deterministic (same inputs → same affinity), so it slots under the
//! engine's rollback alongside the rest of the genetics layer.

/// A set of elemental affinities held as a 3-bit mask (fire | ice | wind).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Affinity(u8);

fn splitmix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A deterministic `[0,1)` draw from `seed`.
fn unit(seed: u64) -> f64 {
    (splitmix(seed) >> 11) as f64 / ((1u64 << 53) as f64)
}

impl Affinity {
    pub const NONE: Affinity = Affinity(0);
    pub const FIRE: Affinity = Affinity(1);
    pub const ICE: Affinity = Affinity(2);
    pub const WIND: Affinity = Affinity(4);
    const MASK: u8 = 0b111;
    const SINGLES: [Affinity; 3] = [Affinity::FIRE, Affinity::ICE, Affinity::WIND];
    /// The affinities offered in the egg picker: none + the three singles. (Multi-
    /// element combos stay the rare wild prize rather than a menu option.)
    pub const CHOOSABLE: [Affinity; 4] =
        [Affinity::NONE, Affinity::FIRE, Affinity::ICE, Affinity::WIND];

    /// Parse a label back into an affinity (inverse of [`label`](Self::label)).
    pub fn from_label(s: &str) -> Option<Affinity> {
        Some(match s {
            "none" => Affinity::NONE,
            "fire" => Affinity::FIRE,
            "ice" => Affinity::ICE,
            "wind" => Affinity::WIND,
            "fire+ice" => Affinity::from_bits(3),
            "fire+wind" => Affinity::from_bits(5),
            "ice+wind" => Affinity::from_bits(6),
            "fire+ice+wind" => Affinity::from_bits(7),
            _ => return None,
        })
    }

    pub fn from_bits(bits: u8) -> Affinity {
        Affinity(bits & Self::MASK)
    }
    pub fn bits(self) -> u8 {
        self.0 & Self::MASK
    }
    pub fn has_fire(self) -> bool {
        self.0 & 1 != 0
    }
    pub fn has_ice(self) -> bool {
        self.0 & 2 != 0
    }
    pub fn has_wind(self) -> bool {
        self.0 & 4 != 0
    }
    pub fn is_none(self) -> bool {
        self.bits() == 0
    }
    /// How many distinct elements this affinity carries (0–3).
    pub fn count(self) -> u32 {
        self.bits().count_ones()
    }
    /// The union of two affinities (used as the inherited gene pool).
    pub fn union(self, other: Affinity) -> Affinity {
        Affinity((self.0 | other.0) & Self::MASK)
    }

    /// A human-readable label: "none", "fire", "fire+ice", "fire+ice+wind", …
    pub fn label(self) -> &'static str {
        match self.bits() {
            0 => "none",
            1 => "fire",
            2 => "ice",
            3 => "fire+ice",
            4 => "wind",
            5 => "fire+wind",
            6 => "ice+wind",
            _ => "fire+ice+wind",
        }
    }

    /// A wild dragon's affinity. A single element is by far the most common
    /// (~80%); none (~8%), a pair (~9%), and all three (~3%) are the rare cases,
    /// so most dragons breathe one of fire / ice / wind and a multi-element
    /// arsenal is a prize.
    pub fn from_seed(seed: u64) -> Affinity {
        let roll = unit(seed);
        if roll < 0.80 {
            // one element
            let pick = (unit(seed ^ 0xA1) * 3.0) as usize % 3;
            Self::SINGLES[pick]
        } else if roll < 0.88 {
            Affinity::NONE
        } else if roll < 0.97 {
            // a pair — drop one of the three at random
            let drop = (unit(seed ^ 0xB2) * 3.0) as usize % 3;
            Affinity(Self::MASK & !Self::SINGLES[drop].0)
        } else {
            Affinity(Self::MASK)
        }
    }

    /// The affinity of an offspring of two parents. Usually it adopts a single
    /// element from the parents' combined pool ("one or the other"); less often it
    /// inherits the whole pool (both / all); and rarely a mutation flips a single
    /// element in or the line goes dormant (none). A pure bloodline therefore
    /// breeds mostly true, while crosses keep multi-element lines uncommon.
    pub fn cross(a: Affinity, b: Affinity, seed: u64) -> Affinity {
        let pool = a.union(b);
        let roll = unit(seed);
        if pool.is_none() {
            // Neither parent had an element: usually still none, rarely a spark.
            return if roll < 0.90 {
                Affinity::NONE
            } else {
                Self::SINGLES[(unit(seed ^ 0xC3) * 3.0) as usize % 3]
            };
        }
        let members: Vec<Affinity> = Self::SINGLES.iter().copied().filter(|e| pool.0 & e.0 != 0).collect();
        if roll < 0.72 {
            // one or the other (a single element drawn from the pool)
            members[(unit(seed ^ 0xD4) * members.len() as f64) as usize % members.len()]
        } else if roll < 0.88 {
            // inherit the full pool (both / all)
            pool
        } else if roll < 0.95 {
            // mutation: a single element from anywhere (may introduce a new one)
            Self::SINGLES[(unit(seed ^ 0xE5) * 3.0) as usize % 3]
        } else {
            // recessive: the line goes dormant this generation
            Affinity::NONE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_labels_and_counts() {
        assert_eq!(Affinity::NONE.count(), 0);
        assert!(Affinity::NONE.is_none());
        assert_eq!(Affinity::FIRE.count(), 1);
        assert_eq!(Affinity::FIRE.label(), "fire");
        let fi = Affinity::FIRE.union(Affinity::ICE);
        assert_eq!(fi.count(), 2);
        assert_eq!(fi.label(), "fire+ice");
        assert!(fi.has_fire() && fi.has_ice() && !fi.has_wind());
        assert_eq!(Affinity::from_bits(0b111).label(), "fire+ice+wind");
    }

    #[test]
    fn deterministic() {
        for s in 0..50u64 {
            assert_eq!(Affinity::from_seed(s), Affinity::from_seed(s));
            assert_eq!(
                Affinity::cross(Affinity::FIRE, Affinity::ICE, s),
                Affinity::cross(Affinity::FIRE, Affinity::ICE, s)
            );
        }
    }

    #[test]
    fn wild_dragons_mostly_have_a_single_element() {
        let (mut none, mut one, mut two, mut three) = (0, 0, 0, 0);
        let n = 20_000u64;
        for s in 0..n {
            match Affinity::from_seed(s).count() {
                0 => none += 1,
                1 => one += 1,
                2 => two += 1,
                _ => three += 1,
            }
        }
        // A single element dominates; none / two / three are each much rarer.
        assert!(one > none * 3 && one > two * 3 && one > three * 3, "one={one} none={none} two={two} three={three}");
        // …but every rare case still actually occurs.
        assert!(none > 0 && two > 0 && three > 0, "none={none} two={two} three={three}");
        // All three is the rarest of all.
        assert!(three < two && three < none, "three={three} two={two} none={none}");
    }

    #[test]
    fn crossbreeding_usually_picks_one_parent_element() {
        // Fire × Ice: offspring should mostly be a single fire OR ice; the full
        // fire+ice pairing is the uncommon prize; none / wind-mutations are rare.
        let (mut single_pool, mut both, mut other) = (0, 0, 0);
        let n = 20_000u64;
        for s in 0..n {
            let c = Affinity::cross(Affinity::FIRE, Affinity::ICE, s);
            match (c, c.count()) {
                (Affinity::FIRE, _) | (Affinity::ICE, _) => single_pool += 1,
                (_, 2) if c == Affinity::FIRE.union(Affinity::ICE) => both += 1,
                _ => other += 1,
            }
        }
        assert!(single_pool > both, "single({single_pool}) should beat both({both})");
        assert!(single_pool > other, "single({single_pool}) should beat other({other})");
        assert!(both > 0, "the multi-element cross still appears");
    }

    #[test]
    fn a_pure_bloodline_breeds_mostly_true() {
        let mut fire = 0;
        let n = 10_000u64;
        for s in 0..n {
            if Affinity::cross(Affinity::FIRE, Affinity::FIRE, s).has_fire() {
                fire += 1;
            }
        }
        // Fire × Fire: the pool is just fire, so the common single-and-full cases
        // both yield fire; only the rare mutation/recessive paths lose it.
        assert!(fire as f64 / n as f64 > 0.8, "fire kept in {fire}/{n}");
    }
}
