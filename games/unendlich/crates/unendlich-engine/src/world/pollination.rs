//! Pollination — how genes move between plants.
//!
//! A plant genome ([`crate::world::plant_genetics`]) only matters to a *forest*
//! if it can travel: a tall, fire-resistant, or exotic lineage has to spread, and
//! distant plants have to be able to cross. The carrier is a **pollinator** — a
//! bee or a songbird — that picks up pollen at one flower and deposits it at the
//! next, fertilizing a Mendelian [`PlantGenome::breed`] across the gap.
//!
//! This module is the pure, deterministic core of that handoff: a pollinator
//! carries one dab of [`Pollen`], and [`visit`]ing a flower either picks up its
//! pollen (first stop) or — if the carried pollen came from a *different* plant —
//! cross-pollinates, yielding the offspring genome a seed/fruit will carry. The
//! pollinator's flight, foraging, and lifecycle live in the creature systems;
//! this is just the genetic transaction at each flower, so it unit-tests with no
//! world.
//!
//! Self-pollination (carried pollen is from the very flower being visited) makes
//! no offspring — outcrossing needs two distinct parents, which is exactly what
//! gives forests their churn of new combinations.

use crate::world::plant_genetics::PlantGenome;

/// A dab of pollen a pollinator carries between flowers: the donor's genome plus
/// a stable id of the plant it came from (so a flower can't self-cross via its
/// own carried load).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pollen {
    pub genome: PlantGenome,
    /// Stable identity of the donor plant (e.g. its position/plant seed).
    pub source: u64,
}

/// The result of a pollinator visiting one flowering plant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Visit {
    /// What the pollinator carries away — always this flower's pollen, ready for
    /// the next stop.
    pub carried: Pollen,
    /// A fertilized offspring genome when cross-pollination occurred (the carried
    /// pollen came from a different plant). This is the genome a resulting
    /// seed/fruit carries. `None` on a first visit or a self-cross.
    pub offspring: Option<PlantGenome>,
}

/// A pollinator visits a flowering plant.
///
/// - Arriving with pollen from a **different** plant fertilizes this flower →
///   `offspring = donor.breed(this_flower, seed)`.
/// - It always leaves carrying **this** flower's pollen for the next visit.
/// - Arriving empty-handed, or with this flower's own pollen, makes no offspring.
pub fn visit(
    carried: Option<Pollen>,
    flower_genome: &PlantGenome,
    flower_source: u64,
    seed: u64,
) -> Visit {
    let offspring = match carried {
        Some(p) if p.source != flower_source => Some(p.genome.breed(flower_genome, seed)),
        _ => None,
    };
    Visit {
        carried: Pollen { genome: *flower_genome, source: flower_source },
        offspring,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flower(seed: u64) -> (PlantGenome, u64) {
        (PlantGenome::from_seed(seed), seed)
    }

    #[test]
    fn first_visit_picks_up_pollen_and_makes_nothing() {
        let (a, a_src) = flower(1);
        let v = visit(None, &a, a_src, 100);
        assert!(v.offspring.is_none());
        assert_eq!(v.carried.source, a_src);
        assert_eq!(v.carried.genome, a);
    }

    #[test]
    fn cross_pollination_breeds_the_two_parents() {
        let (a, a_src) = flower(1);
        let (b, b_src) = flower(2);
        // Pollinator already carries A, now visits B.
        let v = visit(Some(Pollen { genome: a, source: a_src }), &b, b_src, 777);
        assert_eq!(v.offspring, Some(a.breed(&b, 777)));
        // And it leaves carrying B for the next flower.
        assert_eq!(v.carried.source, b_src);
        assert_eq!(v.carried.genome, b);
    }

    #[test]
    fn self_pollination_makes_no_offspring() {
        let (a, a_src) = flower(1);
        // Carrying A's pollen and landing back on A: no outcross.
        let v = visit(Some(Pollen { genome: a, source: a_src }), &a, a_src, 5);
        assert!(v.offspring.is_none());
        assert_eq!(v.carried.source, a_src);
    }

    #[test]
    fn a_two_hop_flight_yields_a_cross() {
        // Bee starts empty, visits A, then B → offspring is breed(A, B).
        let (a, a_src) = flower(10);
        let (b, b_src) = flower(20);
        let after_a = visit(None, &a, a_src, 0);
        assert!(after_a.offspring.is_none());
        let after_b = visit(Some(after_a.carried), &b, b_src, 42);
        assert_eq!(after_b.offspring, Some(a.breed(&b, 42)));
    }

    #[test]
    fn is_deterministic() {
        let (a, a_src) = flower(3);
        let (b, b_src) = flower(4);
        let c = Some(Pollen { genome: a, source: a_src });
        for s in [1u64, 50, 9_999, u64::MAX] {
            assert_eq!(visit(c, &b, b_src, s), visit(c, &b, b_src, s));
        }
    }
}
