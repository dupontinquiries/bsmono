//! `sim::egg_genome` — a creature's heritable [`Genome`] and the egg item it
//! travels in.
//!
//! A genome is eight `u8` traits. An *egg* is the item form of a genome: laid by
//! breeding, carried in the world, hatched back into a creature. The packing is
//! a plain byte copy ([`to_egg`] / [`from_egg`]) so the round-trip is exact.
//!
//! [`breed`] mixes two parents into a child: each of the eight traits is taken
//! from one parent or the other, chosen by a deterministic hash of the breeding
//! `seed` and the trait index. Same parents and same seed always yield the same
//! child (so it rolls back and replays identically); different seeds explore
//! different combinations. No clock, no RNG, no allocation — pure.

/// A creature's heritable traits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Genome {
    /// Eight opaque trait bytes (the meaning of each is decided elsewhere).
    pub traits: [u8; 8],
}

/// Pack a genome into its egg byte form.
pub fn to_egg(g: &Genome) -> [u8; 8] {
    g.traits
}

/// Unpack an egg's bytes back into a genome. Inverse of [`to_egg`].
pub fn from_egg(b: [u8; 8]) -> Genome {
    Genome { traits: b }
}

/// SplitMix64-style finalizer: scrambles a `u64` into a well-mixed `u64`.
/// Pure and deterministic; used only to derive per-trait parent choices.
fn mix64(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^= x >> 31;
    x
}

/// Breed two parents into a child genome.
///
/// For each trait index `i`, a hash of `(seed, i)` decides whether the child
/// takes `a`'s or `b`'s byte at that position. The result is therefore always
/// per-trait one of the two parents (so every child byte lies between the
/// parents' bytes, inclusive), and is fully determined by `(a, b, seed)`.
pub fn breed(a: &Genome, b: &Genome, seed: u64) -> Genome {
    const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut traits = [0u8; 8];
    let mut i = 0;
    while i < 8 {
        let h = mix64(seed ^ (i as u64).wrapping_mul(GOLDEN));
        // use a high bit so adjacent indices/seeds decorrelate
        traits[i] = if (h >> 33) & 1 == 0 {
            a.traits[i]
        } else {
            b.traits[i]
        };
        i += 1;
    }
    Genome { traits }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn rand_genome(seed: &mut u32) -> Genome {
        let mut t = [0u8; 8];
        let mut i = 0;
        while i < 8 {
            t[i] = (lcg(seed) & 0xFF) as u8;
            i += 1;
        }
        Genome { traits: t }
    }

    #[test]
    fn egg_round_trip_is_identity() {
        let mut seed = 0xE66_u32;
        for _ in 0..2000 {
            let g = rand_genome(&mut seed);
            assert_eq!(from_egg(to_egg(&g)), g);
        }
    }

    #[test]
    fn child_traits_come_from_a_parent() {
        let mut seed = 0xC41D_u32;
        for _ in 0..2000 {
            let a = rand_genome(&mut seed);
            let b = rand_genome(&mut seed);
            let s = (lcg(&mut seed) as u64) << 20 | lcg(&mut seed) as u64;
            let c = breed(&a, &b, s);
            for i in 0..8 {
                assert!(
                    c.traits[i] == a.traits[i] || c.traits[i] == b.traits[i],
                    "trait {} is neither parent's",
                    i
                );
            }
        }
    }

    #[test]
    fn child_traits_lie_between_parents() {
        let mut seed = 0xB077_u32;
        for _ in 0..2000 {
            let a = rand_genome(&mut seed);
            let b = rand_genome(&mut seed);
            let s = lcg(&mut seed) as u64;
            let c = breed(&a, &b, s);
            for i in 0..8 {
                let lo = a.traits[i].min(b.traits[i]);
                let hi = a.traits[i].max(b.traits[i]);
                assert!(c.traits[i] >= lo && c.traits[i] <= hi);
            }
        }
    }

    #[test]
    fn same_parents_and_seed_give_same_child() {
        let mut seed = 0x5A3E_u32;
        for _ in 0..2000 {
            let a = rand_genome(&mut seed);
            let b = rand_genome(&mut seed);
            let s = lcg(&mut seed) as u64;
            assert_eq!(breed(&a, &b, s), breed(&a, &b, s));
        }
    }

    #[test]
    fn different_seeds_can_give_different_children() {
        // parents that differ in every trait, so a flipped pick is observable
        let a = Genome { traits: [0; 8] };
        let b = Genome { traits: [255; 8] };
        let base = breed(&a, &b, 0);
        let mut saw_difference = false;
        let mut s = 1u64;
        while s < 256 {
            if breed(&a, &b, s) != base {
                saw_difference = true;
                break;
            }
            s += 1;
        }
        assert!(saw_difference, "seed never changed the child");
    }

    #[test]
    fn identical_parents_breed_that_genome_for_any_seed() {
        let mut seed = 0xDDDD_u32;
        for _ in 0..1000 {
            let g = rand_genome(&mut seed);
            let s = lcg(&mut seed) as u64;
            // when both parents are equal, the child must equal them regardless
            assert_eq!(breed(&g, &g, s), g);
        }
    }
}
