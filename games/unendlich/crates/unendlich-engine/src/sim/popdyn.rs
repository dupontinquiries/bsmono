//! Population dynamics — a tiny, deterministic colony simulation that ties the
//! feeding and genetics models together so we can *test the ecosystem's shape*:
//! a fed colony breeds and grows, a starved one dies out, and eating actually
//! draws down the available grass. It's the harness behind "count the entities
//! in a region after N seconds" — here the region is abstract (a grass pool)
//! rather than world voxels, which keeps it bit-deterministic and fast.
//!
//! Pure. The world version (real grass blocks under real roamers) is the bin
//! step; the rules it must obey are pinned here.

use crate::genetics::{sex_from_seed, Genome, Sex};
use crate::sim::feeding::{try_mate, Vitality, GRASS_FOOD_VALUE};

/// One member of a colony.
#[derive(Debug, Clone)]
pub struct Member {
    pub vit: Vitality,
    pub genome: Genome,
    pub sex: Sex,
}

/// A colony grazing a shared grass pool.
#[derive(Debug, Clone)]
pub struct Colony {
    pub members: Vec<Member>,
    /// Available grass (food units). Each bite costs [`GRASS_FOOD_VALUE`].
    pub grass: f32,
    rng: u64,
}

impl Colony {
    /// `n` full-energy founders with varied genomes and ~50/50 sexes.
    pub fn seeded(n: usize, seed: u64) -> Colony {
        let members = (0..n)
            .map(|i| {
                let s = seed ^ (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                Member {
                    vit: Vitality::full(),
                    genome: Genome::from_seed(s),
                    sex: sex_from_seed(s),
                }
            })
            .collect();
        Colony { members, grass: 0.0, rng: seed | 1 }
    }

    pub fn population(&self) -> usize {
        self.members.len()
    }

    /// Advance one step of `dt` seconds: grass regrows, each member eats a bite
    /// if any grass remains, everyone burns energy, the starved die, then
    /// adjacent opposite-sex well-fed pairs breed (population capped at `cap`).
    pub fn step(&mut self, dt: f32, grass_regen: f32, decay_per_sec: f32, cap: usize) {
        self.grass += grass_regen * dt;

        // Graze + metabolize.
        for m in self.members.iter_mut() {
            if self.grass >= GRASS_FOOD_VALUE {
                m.vit.feed(GRASS_FOOD_VALUE);
                self.grass -= GRASS_FOOD_VALUE;
            }
            m.vit.tick(dt, decay_per_sec);
        }

        // The starved die.
        self.members.retain(|m| !m.vit.is_starving());

        // Breed adjacent opposite-sex fed pairs.
        let mut offspring: Vec<Member> = Vec::new();
        let cap_room = cap.saturating_sub(self.members.len());
        let n = self.members.len();
        let mut rng = self.rng;
        let mut i = 0;
        while i + 1 < n && offspring.len() < cap_room {
            let (a_part, b_part) = self.members.split_at_mut(i + 1);
            let a = &mut a_part[i];
            let b = &mut b_part[0];
            rng ^= rng >> 30;
            rng = rng.wrapping_mul(0xBF58_476D_1CE4_E5B9);
            rng ^= rng >> 27;
            if let Some(g) = try_mate(a.sex, b.sex, &mut a.vit, &mut b.vit, &a.genome, &b.genome, rng) {
                offspring.push(Member {
                    vit: Vitality::new(0.5),
                    genome: g,
                    sex: sex_from_seed(rng),
                });
            }
            i += 2;
        }
        self.rng = rng | 1;
        self.members.extend(offspring);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_colony_with_no_grass_starves_out() {
        let mut c = Colony::seeded(8, 1234);
        // No regrowth, steady metabolism. Run ~40 simulated seconds.
        for _ in 0..400 {
            c.step(1.0, 0.0, 0.1, 256);
        }
        assert_eq!(c.population(), 0, "without food the whole colony dies");
    }

    #[test]
    fn a_well_fed_colony_breeds_and_grows() {
        let mut c = Colony::seeded(10, 99);
        let start = c.population();
        // Ample grass each step so everyone stays fed; run a while.
        for _ in 0..120 {
            c.step(1.0, 50.0, 0.1, 500);
        }
        assert!(c.population() > start, "with plenty of food the colony grows ({start} -> {})", c.population());
    }

    #[test]
    fn eating_draws_down_the_grass() {
        let mut c = Colony::seeded(4, 7);
        c.grass = 10.0;
        let before = c.grass;
        // One step with no regrowth: four mouths each take a bite.
        c.step(1.0, 0.0, 0.01, 256);
        let eaten = before - c.grass;
        assert!((eaten - 4.0 * GRASS_FOOD_VALUE).abs() < 1e-5, "4 members ate 4 bites (ate {eaten})");
    }

    #[test]
    fn starvation_outruns_breeding_when_food_is_scarce() {
        // A trickle of grass — not enough to sustain the colony. It should shrink,
        // not grow, over time (the population can't outbreed scarcity).
        let mut c = Colony::seeded(12, 555);
        let start = c.population();
        for _ in 0..300 {
            c.step(1.0, 0.3, 0.1, 500); // ~1.2 bites/step shared among many
        }
        assert!(c.population() < start, "scarce food shrinks the colony ({start} -> {})", c.population());
    }
}
